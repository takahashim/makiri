//! The Ruby C calls that can RAISE - every `longjmp` this extension can start -
//! and each one made under `rb_protect`, so the raise comes back as `Err`.
//!
//! A raise is `longjmp`, which crosses Rust frames without running their
//! destructors, and crossing a frame that has one (or a `catch_unwind`) is
//! undefined behaviour. So no raising call is made anywhere but here, and here
//! every one runs inside [`call`]: a bare `rb_protect` whose `extern "C"`
//! thunk holds nothing but that one C call and the `Copy` values it reads. The
//! only frame a raise crosses is the thunk's own, which owns nothing. A
//! caller gets a `Result` and owns whatever it likes across the call.
//! `rake unsafe:boundaries` pins that these C names appear nowhere else in the
//! crate, so a new raising call is a new wrapper here, with its raise stated.
//!
//! The value-level wrappers are SAFE: they take and return `bridge::rb`'s
//! typed values (`RString`, `RArray`, `RHash`, `Value`, ...), slices and the
//! bridge's `Encoding`, so a live value of the right kind is the type's
//! promise, not the caller's - under the rule every Ruby extension keeps, that
//! a Ruby value stays on the stack (CLAUDE.md, "A VALUE inside a wrapped
//! object"). One that
//! takes no Ruby value takes `&Ruby`, the proof it runs on a Ruby thread with
//! the GVL (`Ruby` is `!Send`, so a GVL-released body cannot carry one). What
//! still checks a value's kind or a bound is done here, before the C call,
//! where the C function would not check it. The wrappers over raw memory -
//! typed data and Ruby's allocator, and a String of unwritten bytes - stay
//! `unsafe`, with their contracts stated.
//!
//! What is NOT here: calls that cannot raise and touch no other value
//! (`rb_gc_mark`, the encoding singletons, `rb_typeddata_is_kind_of`,
//! `rb_attr_get`, `ruby_xfree`), the stable API's inline reads
//! (`bridge::rb::value`), and the GC callbacks (`bridge::typed`). Together
//! with those, this file is every C function of Ruby's that Makiri calls.

#![allow(unsafe_code)]
/* Every wrapper's contract is the module's, stated once above. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long, c_void};

use crate::nupur::rb::value::AsRawValue;
use crate::nupur::rb::{
    Error, ExceptionClass, Float, Integer, RArray, RClass, RHash, RModule, RString, Range, Ruby,
    Symbol, Value,
};
use rb_sys::{rb_data_type_t, ID, VALUE};

use crate::nupur::encoding::Encoding;

/// The raise sweep's hook (`script/check_raise_injection.rb`): in an
/// `alloc-inject` build the armed allocating call below raises `NoMemoryError`
/// in place of its allocation, as a real failure would - so the sweep can make
/// every one of them raise in turn. Nothing in any other build. Called inside
/// the thunk, so the raise is protected like the call's. Whether a call is
/// armed is the extension's to answer ([`set_raise_injector`]).
#[inline(always)]
unsafe fn injected_raise() {
    #[cfg(feature = "alloc-inject")]
    if RAISE_INJECTOR.get().is_some_and(|armed| armed()) {
        /* Raises without allocating: Ruby keeps the exception preallocated. */
        unsafe { rb_sys::rb_memerror() }
    }
}

/// The function that answers whether the next allocating call raises.
#[cfg(feature = "alloc-inject")]
static RAISE_INJECTOR: std::sync::OnceLock<fn() -> bool> = std::sync::OnceLock::new();

/// Register the raise sweep's answer, once, before any method runs (the
/// extension's `Init_`); a second registration is ignored.
#[cfg(feature = "alloc-inject")]
pub fn set_raise_injector(armed: fn() -> bool) {
    let _ = RAISE_INJECTOR.set(armed);
}

/// Run `f` - one C call, and the `Copy` values it reads - under a bare
/// `rb_protect`; a raise comes back as `Err`.
///
/// `F: Copy` and `T: Copy` are what keep the thunk's frame free of anything a
/// skipped destructor would leak: a closure is `Copy` only when everything it
/// captures is, so it owns nothing, and neither does its result. There is no
/// `catch_unwind` here (a raise crossing one is undefined behaviour); a panic
/// in the thunk would abort at its `extern "C"` boundary, and the closures
/// below hold no Rust that panics. The one C call that calls back into Rust,
/// `rb_hash_foreach` ([`hash_foreach`]), latches its callback's panic inside
/// the callback, and the callback raises nothing.
#[inline]
fn call<T: Copy, F: FnOnce() -> T + Copy>(f: F) -> Result<T, Error> {
    struct Slot<F, T> {
        f: F,
        out: Option<T>,
    }
    unsafe extern "C" fn thunk<T: Copy, F: FnOnce() -> T + Copy>(arg: VALUE) -> VALUE {
        crate::nupur::no_unwind::must_not_unwind::<crate::nupur::no_unwind::ProtectThunk, _>(|| {
            // SAFETY: `arg` is the `Slot` `call` passed, live for the call.
            let slot = unsafe { &mut *(arg as *mut Slot<F, T>) };
            slot.out = Some((slot.f)());
            rb_sys::Qnil as VALUE
        })
    }
    let mut slot = Slot { f, out: None };
    let mut state: c_int = 0;
    // SAFETY: the thunk reads only `slot`, which outlives the call.
    unsafe {
        rb_sys::rb_protect(
            Some(thunk::<T, F>),
            &mut slot as *mut Slot<F, T> as VALUE,
            &mut state,
        );
    }
    match (state, slot.out) {
        (0, Some(out)) => Ok(out),
        (0, None) => Err(crate::nupur::helpers::internal_error(
            "a protected call did not run",
        )),
        (state, _) => Err(Error::from_state(state)),
    }
}

/* ---- what the value-level wrappers share ---- */

/// The live value a wrapped C call returned, as a `T` - checked to be one, so
/// a C function that answered with another kind is an error, not a value read
/// as the wrong type.
#[inline]
fn typed<T>(v: VALUE, of: fn(Value) -> Option<T>, what: &'static str) -> Result<T, Error> {
    // SAFETY: `v` is what a Ruby C call just returned: a live value, on this
    // frame's stack until it is converted.
    /* `what` is the whole message, a literal: this runs on the path that
     * raises a caught panic, which must stay `fmt`-free (`rake no_unwind`). */
    of(unsafe { Value::from_raw(v) }).ok_or_else(|| crate::nupur::helpers::internal_error(what))
}

/// A length for the C API's `long`; `ArgumentError` for one it cannot hold.
#[inline]
fn c_len(n: usize) -> Result<c_long, Error> {
    c_long::try_from(n).map_err(|_| crate::nupur::helpers::arg_error("string too large"))
}

/// A String's length, read without a call.
#[inline]
fn rstring_len(s: RString) -> usize {
    use rb_sys::stable_api::StableApiDefinition;
    // SAFETY: `s` is a live String, by type.
    unsafe { rb_sys::stable_api::get_default().rstring_len(s.as_raw()) as usize }
}

/* ---- what bridge::rb's values and errors are made of ---- */

/// The `rb_protect` state of a raise (`TAG_RAISE` in CRuby's `vm_core.h`).
/// Not part of the documented API - the state is called opaque - but it is
/// how a raise is told from a `throw` or a `break`, whose state must be
/// resumed untouched; magnus reads the same value. Unchanged since 1.9.
pub const TAG_RAISE: c_int = 6;

/// `$!`, taken: the exception a raise left there, with `$!` set back to
/// `nil` (`rb_errinfo`, `rb_set_errinfo`). Raises nothing.
pub fn take_errinfo() -> Value {
    // SAFETY: under the GVL (a protected call just returned); both read or
    // write the current thread's error info.
    unsafe {
        let exc = rb_sys::rb_errinfo();
        rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE);
        Value::from_raw(exc)
    }
}

/// `rb_jump_tag`: resume the non-local exit `state` stands for. Raises, by
/// design - only `Error::raise` calls it, with nothing owned.
pub fn jump_tag(state: c_int) -> ! {
    // SAFETY: `state` is one `rb_protect` reported; resuming it is the point.
    unsafe { rb_sys::rb_jump_tag(state) }
}

/// `rb_exc_raise`: raise `exc`. Raises, by design - only `Error::raise` calls
/// it, with nothing owned.
pub fn exc_raise(exc: Value) -> ! {
    // SAFETY: `exc` is a live exception object.
    unsafe { rb_sys::rb_exc_raise(exc.as_raw()) }
}

/// An exception of `class` with the UTF-8 message `msg`: `rb_exc_new_str`,
/// which allocates and runs the class's `initialize`, and so raises.
pub fn exception_new(ruby: &Ruby, class: ExceptionClass, msg: &[u8]) -> Result<Value, Error> {
    let s = utf8_str_new(ruby, msg)?.as_raw();
    let class = class.as_raw();
    let exc = call(|| unsafe {
        injected_raise();
        rb_sys::rb_exc_new_str(class, s)
    })?;
    // SAFETY: the object just made.
    Ok(unsafe { Value::from_raw(exc) })
}

/// Whether `v` is a class or a module - what `rb_obj_is_kind_of` and
/// `rb_class_inherited_p` require, and raise `TypeError` without.
#[inline]
fn is_module(v: Value) -> bool {
    RClass::from_value(v).is_some() || RModule::from_value(v).is_some()
}

/// `rb_obj_is_kind_of`: an instance of `class` or of one deriving from it?
/// `false` when `class` is not a class or module, rather than the
/// `TypeError` the C function raises. Runs no Ruby code.
#[inline]
pub fn obj_is_kind_of(v: Value, class: Value) -> bool {
    if !is_module(class) {
        return false;
    }
    // SAFETY: both live; `class` is a class or module, so this raises nothing.
    unsafe { rb_sys::rb_obj_is_kind_of(v.as_raw(), class.as_raw()) == rb_sys::Qtrue as VALUE }
}

/// `rb_obj_classname`: the name of `v`'s class, handed to `f` as bytes. Raises
/// `NoMemoryError`: an anonymous class's name (`#<Class:0x...>`) is a String
/// made on each call. That String is referenced by nothing once the call
/// returns, so `f` reads it before anything can allocate - it must run no
/// Ruby.
pub fn with_classname<R>(v: Value, f: impl FnOnce(&[u8]) -> R) -> Result<R, Error> {
    let v = v.as_raw();
    let p = call(|| unsafe {
        injected_raise();
        rb_sys::rb_obj_classname(v)
    })?;
    if p.is_null() {
        return Ok(f(b"Object"));
    }
    // SAFETY: a NUL-terminated C string owned by the class, or by the
    // temporary name String, which no collection can free before `f` is done
    // (`f` allocates nothing).
    Ok(f(unsafe { core::ffi::CStr::from_ptr(p) }.to_bytes()))
}

/// `rb_class_inherited_p`: is `sub` `sup` or a descendant of it? `false`
/// when either is not a class or module. Runs no Ruby code.
#[inline]
pub fn class_inherits(sub: Value, sup: Value) -> bool {
    if !is_module(sub) || !is_module(sup) {
        return false;
    }
    // SAFETY: both live modules, so this raises nothing.
    unsafe { rb_sys::rb_class_inherited_p(sub.as_raw(), sup.as_raw()) == rb_sys::Qtrue as VALUE }
}

/// `rb_ary_entry`: element `i` (negative counts from the end), `nil` past
/// either end. Raises nothing.
#[inline]
pub fn ary_entry(a: RArray, i: c_long) -> Value {
    // SAFETY: `a` is a live Array; the read is bounds-checked by Ruby.
    unsafe { Value::from_raw(rb_sys::rb_ary_entry(a.as_raw(), i)) }
}

/// `rb_hash_lookup2` at `key`, `Ok(None)` for no entry. The key is a Symbol
/// or a String, hashed and compared in C - but a key already IN the Hash may
/// be any object, and one whose hash collides is compared with its `eql?`,
/// the caller's Ruby, which can raise or `throw`. That comes back as `Err`,
/// to be passed on: read as "no entry", the exception would be lost and the
/// keyword taken as absent. In the raise sweep's set (`injected_raise`), so
/// that path is run.
pub fn hash_lookup(h: RHash, key: Value) -> Result<Option<Value>, Error> {
    const UNDEF: VALUE = rb_sys::Qundef as VALUE;
    let (h, key) = (h.as_raw(), key.as_raw());
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_lookup2(h, key, UNDEF)
    })?;
    // SAFETY: a value of the Hash's, or `Qundef`, which is not handed out.
    Ok((v != UNDEF).then(|| unsafe { Value::from_raw(v) }))
}

/// `rb_range_values`: a Range's ends and whether it excludes its end. For a
/// Range (or a subclass's instance) the fields are read, and no Ruby runs.
pub fn range_values(r: Range) -> Result<(Value, Value, bool), Error> {
    let r = r.as_raw();
    let (mut beg, mut end, mut excl): (VALUE, VALUE, c_int) = (0, 0, 0);
    let (pb, pe, px) = (
        &mut beg as *mut VALUE,
        &mut end as *mut VALUE,
        &mut excl as *mut c_int,
    );
    call(|| unsafe { rb_sys::rb_range_values(r, pb, pe, px) })?;
    // SAFETY: the Range's own ends, live while it is.
    Ok(unsafe { (Value::from_raw(beg), Value::from_raw(end), excl != 0) })
}

/// The constant `name` of `outer` (`rb_const_get`): raises `NameError` for
/// none, and an autoload runs Ruby code - used only for a class Makiri's own
/// Ruby defines.
pub fn const_get(ruby: &Ruby, outer: Value, name: &core::ffi::CStr) -> Result<Value, Error> {
    let id = intern_cstr(ruby, name)?;
    let outer = outer.as_raw();
    let v = call(|| unsafe { rb_sys::rb_const_get(outer, id) })?;
    // SAFETY: the constant's value, live while it is bound.
    Ok(unsafe { Value::from_raw(v) })
}

/// `recv.name(*args)` (`rb_funcallv`): the caller's Ruby, which is why each
/// call of it is allowed where it is made (`rake dylint`'s
/// `runs_callers_ruby`) - the one use is
/// `XML::Namespace.new`, Makiri's own Data class, whose `new` takes the
/// arguments as Data's does.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "the one call of a method: XML::Namespace.new, Makiri's own Data class"
    )
)]
pub fn call_method(
    ruby: &Ruby,
    recv: Value,
    name: &core::ffi::CStr,
    args: &[Value],
) -> Result<Value, Error> {
    let id = intern_cstr(ruby, name)?;
    let argc = c_int::try_from(args.len())
        .map_err(|_| crate::nupur::helpers::arg_error("too many arguments"))?;
    let recv = recv.as_raw();
    /* `Value` is transparent over `VALUE`, so the slice is the C array. */
    let argv = args.as_ptr() as *const VALUE;
    let v = call(|| unsafe { rb_sys::rb_funcallv(recv, id, argc, argv) })?;
    // SAFETY: what the method returned, live on this frame.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_module_new`: an anonymous module. Raises `NoMemoryError`.
pub fn module_new(_: &Ruby) -> Result<RModule, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_module_new()
    })?;
    typed(v, RModule::from_value, "a C call made no Module")
}

/// `rb_big2ll`: raises `RangeError` past an `i64`.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_longlong is i64 here, not everywhere"
)]
pub fn big2ll(n: Integer) -> Result<i64, Error> {
    let n = n.as_raw();
    Ok(call(|| unsafe { rb_sys::rb_big2ll(n) })? as i64)
}

/// `rb_big2ull`: raises `RangeError` past a `u64`.
#[allow(
    clippy::unnecessary_cast,
    reason = "c_ulonglong is u64 here, not everywhere"
)]
pub fn big2ull(n: Integer) -> Result<u64, Error> {
    let n = n.as_raw();
    Ok(call(|| unsafe { rb_sys::rb_big2ull(n) })? as u64)
}

/* ---- definitions, at init ---- */

/// `rb_define_class_under`: raises `TypeError` when `name` is bound to
/// something else, or to a class with another superclass.
pub fn define_class_under(
    outer: Value,
    name: &core::ffi::CStr,
    sup: RClass,
) -> Result<Value, Error> {
    let (outer, name, sup) = (outer.as_raw(), name.as_ptr(), sup.as_raw());
    let v = call(|| unsafe { rb_sys::rb_define_class_under(outer, name, sup) })?;
    // SAFETY: the class just defined, live (it is a constant now).
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_define_module_under`: raises `TypeError` when `name` is bound to
/// something that is not a module.
pub fn define_module_under(outer: Value, name: &core::ffi::CStr) -> Result<Value, Error> {
    let (outer, name) = (outer.as_raw(), name.as_ptr());
    let v = call(|| unsafe { rb_sys::rb_define_module_under(outer, name) })?;
    // SAFETY: as `define_class_under`.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_define_module`: a top-level module; raises as `define_module_under`.
pub fn define_module(name: &core::ffi::CStr) -> Result<Value, Error> {
    let name = name.as_ptr();
    let v = call(|| unsafe { rb_sys::rb_define_module(name) })?;
    // SAFETY: as `define_class_under`.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_const_set`: raises `FrozenError` on a frozen namespace.
pub fn const_set(ruby: &Ruby, outer: Value, name: &core::ffi::CStr, v: Value) -> Result<(), Error> {
    let id = intern_cstr(ruby, name)?;
    let (outer, v) = (outer.as_raw(), v.as_raw());
    call(|| unsafe { rb_sys::rb_const_set(outer, id, v) })
}

/// `rb_include_module`: raises `ArgumentError` for a cyclic include,
/// `FrozenError` on a frozen class.
pub fn include_module(klass: Value, module: Value) -> Result<(), Error> {
    let (klass, module) = (klass.as_raw(), module.as_raw());
    call(|| unsafe { rb_sys::rb_include_module(klass, module) })
}

/// `rb_undef_alloc_func`. Raises nothing.
pub fn undef_alloc_func(klass: RClass) {
    // SAFETY: `klass` is a live class.
    unsafe { rb_sys::rb_undef_alloc_func(klass.as_raw()) }
}

/// `rb_define_method` / `_private_method` / `_singleton_method`: raise
/// `FrozenError` on a frozen target.
pub(in crate::nupur) fn define_method(
    target: Value,
    name: &core::ffi::CStr,
    func: super::rb::method::Func,
    visibility: super::rb::class::Visibility,
) -> Result<(), Error> {
    use super::rb::class::Visibility;
    let (target, name, ptr, arity) = (target.as_raw(), name.as_ptr(), Some(func.ptr), func.arity);
    match visibility {
        Visibility::Public => {
            call(|| unsafe { rb_sys::rb_define_method(target, name, ptr, arity) })
        }
        Visibility::Private => {
            call(|| unsafe { rb_sys::rb_define_private_method(target, name, ptr, arity) })
        }
        Visibility::Singleton => {
            call(|| unsafe { rb_sys::rb_define_singleton_method(target, name, ptr, arity) })
        }
        Visibility::ModuleFunction => {
            call(|| unsafe { rb_sys::rb_define_module_function(target, name, ptr, arity) })
        }
    }
}

/// `rb_gc_register_mark_object`: raises `NoMemoryError` when the root table
/// cannot grow.
pub fn gc_register_mark_object(v: Value) -> Result<(), Error> {
    let v = v.as_raw();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_gc_register_mark_object(v)
    })
}

/// `rb_obj_freeze`. Raises nothing.
pub fn obj_freeze(v: Value) {
    // SAFETY: `v` is a live object.
    unsafe { rb_sys::rb_obj_freeze(v.as_raw()) };
}

/// `rb_exc_fatal`: raise `exc` as Ruby's `fatal` - for a panic in
/// `Init_makiri`, where a half-defined extension must not be used. Raises,
/// by design.
pub fn exc_fatal(exc: Value) -> ! {
    // SAFETY: `exc` is a live exception object.
    unsafe { rb_sys::rb_exc_fatal(exc.as_raw()) }
}

/* ---- blocks and Hash walks ---- */

/// `rb_block_given_p`: whether the method running now was given a block.
/// Raises nothing and runs no Ruby code; like [`keyword_given`], it reads
/// the current frame.
#[inline]
pub fn block_given(_: &Ruby) -> bool {
    // SAFETY: under the GVL (`&Ruby`); a read of the current control frame.
    unsafe { rb_sys::rb_block_given_p() != 0 }
}

/// `rb_yield`: runs the caller's block - Ruby code, which can raise, `break`,
/// `throw` or `return`. Every one of those comes back as `Err` (the
/// non-exception exits as the jump `rb_protect` caught), and is raised again
/// at the method's return, so it reaches the block's caller as it left.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "NodeSet#each's block, the point of the method"
    )
)]
pub fn yield_value(_: &Ruby, v: Value) -> Result<Value, Error> {
    let v = v.as_raw();
    let out = call(|| unsafe { rb_sys::rb_yield(v) })?;
    // SAFETY: what the block returned, live on this frame.
    Ok(unsafe { Value::from_raw(out) })
}

/// `rb_hash_foreach` over `h`, `visit` answering after each pair whether to
/// go on. Raises `RuntimeError` when the Hash is changed in a way the walk
/// cannot survive (a key added or the table rehashed).
///
/// `visit` is Rust called from C, inside `rb_hash_foreach`'s frames, so a
/// panic in it must not unwind into them: it is latched here - the walk
/// stops - and resumed once the C frames are gone. It never raises either:
/// it answers `false` and the caller keeps its own error.
pub fn hash_foreach(h: RHash, visit: &mut dyn FnMut(Value, Value) -> bool) -> Result<(), Error> {
    struct Walk<'a> {
        visit: &'a mut dyn FnMut(Value, Value) -> bool,
        latch: crate::nupur::caught::PanicLatch,
    }
    unsafe extern "C" fn each(k: VALUE, v: VALUE, arg: VALUE) -> c_int {
        const CONTINUE: c_int = rb_sys::st_retval::ST_CONTINUE as c_int;
        const STOP: c_int = rb_sys::st_retval::ST_STOP as c_int;
        crate::nupur::no_unwind::must_not_unwind::<crate::nupur::no_unwind::HashEach, _>(|| {
            // SAFETY: `arg` is the `Walk` `hash_foreach` passed, live for the
            // walk; `k` and `v` are the Hash's, live while it is walked.
            let walk = unsafe { &mut *(arg as *mut Walk<'_>) };
            let (k, v) = unsafe { (Value::from_raw(k), Value::from_raw(v)) };
            let visit = &mut walk.visit;
            if walk.latch.guard(false, || visit(k, v)) {
                CONTINUE
            } else {
                STOP
            }
        })
    }
    let mut walk = Walk {
        visit,
        latch: crate::nupur::caught::PanicLatch::new(),
    };
    let (h, arg) = (h.as_raw(), &mut walk as *mut Walk<'_> as VALUE);
    let walked = call(|| unsafe {
        rb_sys::rb_hash_foreach(h, Some(each), arg);
        rb_sys::Qnil as VALUE
    });
    /* Out of the C frames: safe to unwind from here. */
    walk.latch.resume();
    walked.map(|_| ())
}

/* ---- conversions and checks ---- */

/// `rb_keyword_given_p`: whether the method running now was called with
/// keywords - the one thing `rb_scan_args` asks Ruby when it separates them.
/// Raises nothing and runs no Ruby code. It reads the current frame, so it
/// answers for a method only from that method's own body, before the method
/// calls into Ruby (`glue::args::scan` is called first thing).
#[inline]
pub fn keyword_given(_: &Ruby) -> bool {
    // SAFETY: under the GVL (`&Ruby`); a read of the current control frame.
    unsafe { rb_sys::rb_keyword_given_p() != 0 }
}

/// `rb_sym2str`: raises `NoMemoryError` when a static Symbol's name String is
/// made for the first time. Runs no Ruby code.
#[inline]
pub fn sym2str(sym: Symbol) -> Result<RString, Error> {
    let v = sym.as_raw();
    let s = call(|| unsafe {
        injected_raise();
        rb_sys::rb_sym2str(v)
    })?;
    typed(s, RString::from_value, "a C call made no String")
}

/// `rb_check_frozen`: raises `FrozenError` for a frozen value.
#[inline]
pub fn check_frozen(v: Value) -> Result<(), Error> {
    let v = v.as_raw();
    call(|| unsafe { rb_sys::rb_check_frozen(v) })
}

/// `rb_range_beg_len` with `err = 0` over a collection of `count` elements:
/// `None` for a start outside it, `RangeError` for a bound too large for a
/// `long`. `range` must be a Range whose ends are Integers or nil (the
/// caller's check - `bridge::ruby::integer_range_arg`): an end of any other
/// kind would be converted with its `to_int`.
#[inline]
pub fn range_beg_len(range: Value, count: c_long) -> Result<Option<(c_long, c_long)>, Error> {
    if crate::nupur::rb::Range::from_value(range).is_none() {
        return Err(crate::nupur::helpers::type_error("a Range was expected"));
    }
    let (mut beg, mut len): (c_long, c_long) = (0, 0);
    let (b, l) = (&mut beg as *mut c_long, &mut len as *mut c_long);
    let r = range.as_raw();
    let answer = call(|| unsafe { rb_sys::rb_range_beg_len(r, b, l, count, 0) })?;
    Ok((answer == rb_sys::Qtrue as VALUE).then_some((beg, len)))
}

/// `rb_thread_check_ints`: raises a pending interrupt (`Interrupt`,
/// `Thread#raise`'s exception, a `Timeout`), or ends the thread.
#[inline]
pub fn thread_check_ints(_: &Ruby) -> Result<(), Error> {
    call(|| unsafe { rb_sys::rb_thread_check_ints() })
}

/* ---- symbols ---- */

/// `rb_intern3` of a UTF-8 name: raises `NoMemoryError` when the symbol table
/// cannot grow (interning valid UTF-8 raises nothing else).
#[inline]
pub fn intern(_: &Ruby, name: &str) -> Result<ID, Error> {
    let (p, n) = (name.as_ptr() as *const c_char, c_len(name.len())?);
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern3(p, n, rb_sys::rb_utf8_encoding())
    })
}

/// `rb_intern` of a NUL-terminated name: as [`intern`].
#[inline]
pub fn intern_cstr(_: &Ruby, name: &core::ffi::CStr) -> Result<ID, Error> {
    let p = name.as_ptr();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern(p)
    })
}

/// The Symbol of the UTF-8 `name`: [`intern`] and `rb_id2sym`, which raises
/// `NoMemoryError` when a dynamic Symbol is made.
#[inline]
pub fn sym(ruby: &Ruby, name: &str) -> Result<Symbol, Error> {
    let id = intern(ruby, name)?;
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_id2sym(id)
    })?;
    typed(v, Symbol::from_value, "a C call made no Symbol")
}

/* ---- Strings ---- */

/// `rb_utf8_str_new` of `bytes`: raises `NoMemoryError`. The String is tagged
/// UTF-8 without a check: `bytes` that were not would make a wrong String,
/// never a memory error.
#[inline]
pub fn utf8_str_new(_: &Ruby, bytes: &[u8]) -> Result<RString, Error> {
    let (p, n) = (bytes.as_ptr() as *const c_char, c_len(bytes.len())?);
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_utf8_str_new(p, n)
    })?;
    typed(v, RString::from_value, "a C call made no String")
}

/// A UTF-8 String of exactly `len` bytes, written by `fill` straight into
/// the new String's buffer - one allocation, no intermediate copy. The
/// String's allocation raises `NoMemoryError`; `fill`'s `Err` is returned in
/// its place, as is a `fill` that wrote fewer than `len` bytes - an internal
/// error, since the bytes left would be whatever the allocation held - and
/// the half-written String is left to the GC, unseen.
///
/// `fill` writes through an [`Unwritten`], front to back, so every byte the
/// String holds was written and none past its end can be. The String is
/// reachable from nowhere but this frame, which keeps it alive and in place
/// while `fill` runs (the GC's stack scan pins it), so no Ruby code can see
/// or change it.
///
/// The bytes are tagged UTF-8 without a check; bytes that were not would make
/// a wrong String, never a memory error.
pub fn utf8_str_filled(
    _: &Ruby,
    len: usize,
    fill: impl FnOnce(&mut Unwritten<'_>) -> Result<(), Error>,
) -> Result<RString, Error> {
    let n = c_len(len)?;
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_utf8_str_new(core::ptr::null(), n)
    })?;
    let str = typed(v, RString::from_value, "a C call made no String")?;
    // SAFETY: `str` is the String just made, of `len` bytes (`rstring_ptr`
    // is its buffer, the room `rb_str_new` allocated), referenced only from
    // this frame. Seen as `MaybeUninit`, so nothing reads it unwritten.
    let buf: &mut [core::mem::MaybeUninit<u8>] = unsafe {
        let dst = rb_sys::StableApiDefinition::rstring_ptr(
            rb_sys::stable_api::get_default(),
            str.as_raw(),
        ) as *mut core::mem::MaybeUninit<u8>;
        if len == 0 || dst.is_null() {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(dst, len)
        }
    };
    let mut w = Unwritten { buf, at: 0 };
    fill(&mut w)?;
    if w.at != len {
        return Err(crate::nupur::helpers::internal_error(
            "a String was left part-written",
        ));
    }
    Ok(str)
}

/// The buffer of a String [`utf8_str_filled`] is making, written front to
/// back: the bytes before `at` are written, the rest are not and cannot be
/// read.
pub struct Unwritten<'a> {
    buf: &'a mut [core::mem::MaybeUninit<u8>],
    at: usize,
}

impl Unwritten<'_> {
    /// Append `src`; `false`, with nothing written, when it does not fit.
    #[inline]
    pub fn push(&mut self, src: &[u8]) -> bool {
        let Some(dst) = self
            .at
            .checked_add(src.len())
            .and_then(|end| self.buf.get_mut(self.at..end))
        else {
            return false;
        };
        // SAFETY: `u8` and `MaybeUninit<u8>` have one layout, and every `u8`
        // is a valid `MaybeUninit<u8>`; the view is only read.
        let src: &[core::mem::MaybeUninit<u8>] =
            unsafe { &*(src as *const [u8] as *const [core::mem::MaybeUninit<u8>]) };
        copy_small(dst, src);
        self.at += src.len();
        true
    }

    /// How many bytes are written.
    #[inline]
    pub fn written(&self) -> usize {
        self.at
    }
}

/// `dst.copy_from_slice(src)` for slices of one length, with no `memcpy`
/// call for 16 bytes or fewer: two fixed-size copies that overlap. Text
/// slices are mostly a few bytes. (Makiri's `cbuf::copy_small` is the same
/// copy over `u8`, the one Verus proves.)
#[inline(always)]
#[allow(
    clippy::indexing_slicing,
    reason = "dst and src are one length, at least K"
)]
fn copy_small<T: Copy>(dst: &mut [T], src: &[T]) {
    #[inline(always)]
    fn two<const K: usize, T: Copy>(dst: &mut [T], src: &[T]) {
        let n = src.len();
        dst[..K].copy_from_slice(&src[..K]);
        dst[n - K..n].copy_from_slice(&src[n - K..]);
    }
    match src.len() {
        0 => {}
        1 => dst[0] = src[0],
        2..=3 => two::<2, T>(dst, src),
        4..=7 => two::<4, T>(dst, src),
        8..=16 => two::<8, T>(dst, src),
        _ => dst.copy_from_slice(src),
    }
}

/// `rb_enc_interned_str` of `bytes` in `enc`: raises `NoMemoryError`.
#[inline]
pub fn enc_interned_str(_: &Ruby, bytes: &[u8], enc: Encoding) -> Result<RString, Error> {
    let (p, n, e) = (
        bytes.as_ptr() as *const c_char,
        c_len(bytes.len())?,
        enc.as_raw(),
    );
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_enc_interned_str(p, n, e)
    })?;
    typed(v, RString::from_value, "a C call made no String")
}

/// `rb_str_dup`: raises `NoMemoryError`.
#[inline]
pub fn str_dup(s: RString) -> Result<RString, Error> {
    let v = s.as_raw();
    let d = call(|| unsafe {
        injected_raise();
        rb_sys::rb_str_dup(v)
    })?;
    typed(d, RString::from_value, "a C call made no String")
}

/// `rb_str_subseq`: the `len` bytes of `s` from `beg`, which must lie inside
/// it - checked here, since the C function reads them unchecked. Raises
/// `NoMemoryError`.
#[inline]
pub fn str_subseq(s: RString, beg: usize, len: usize) -> Result<RString, Error> {
    if beg.checked_add(len).is_none_or(|end| end > rstring_len(s)) {
        return Err(crate::nupur::helpers::internal_error(
            "a substring past its String",
        ));
    }
    let (v, b, n) = (s.as_raw(), c_len(beg)?, c_len(len)?);
    let u = call(|| unsafe {
        injected_raise();
        rb_sys::rb_str_subseq(v, b, n)
    })?;
    typed(u, RString::from_value, "a C call made no String")
}

/// `rb_enc_associate`: `s` itself, tagged `enc`. Raises `FrozenError` for a
/// frozen String, and `ArgumentError` for an encoding its bytes cannot carry.
#[inline]
pub fn enc_associate(s: RString, enc: Encoding) -> Result<RString, Error> {
    let (v, e) = (s.as_raw(), enc.as_raw());
    let r = call(|| unsafe { rb_sys::rb_enc_associate(v, e) })?;
    typed(r, RString::from_value, "a C call made no String")
}

/// `rb_str_encode` to `enc` with `flags`: raises `Encoding::ConverterNotFoundError`
/// when Ruby has no converter, and - without replacement flags -
/// `Encoding::InvalidByteSequenceError` / `UndefinedConversionError`.
#[inline]
pub fn str_encode(s: RString, enc: Encoding, flags: c_int) -> Result<RString, Error> {
    let (v, e) = (s.as_raw(), enc.as_raw());
    let r = call(|| unsafe {
        rb_sys::rb_str_encode(
            v,
            rb_sys::rb_enc_from_encoding(e),
            flags,
            rb_sys::Qnil as VALUE,
        )
    })?;
    typed(r, RString::from_value, "a C call made no String")
}

/// `rb_to_encoding` of an `Encoding` or a String: raises `ArgumentError` for
/// an unknown name. Any other value is a `TypeError` here, before the call,
/// which would convert it with its `to_str`.
#[inline]
pub fn to_encoding(ruby: &Ruby, v: Value) -> Result<Encoding, Error> {
    use crate::nupur::rb::ReprValue;
    if !v.is_kind_of(ruby.class_encoding()) && RString::from_value(v).is_none() {
        return Err(crate::nupur::helpers::type_error(
            "an Encoding or a String was expected",
        ));
    }
    let r = v.as_raw();
    let e = call(|| unsafe { rb_sys::rb_to_encoding(r) })?;
    // SAFETY: Ruby's encodings live for the process.
    Ok(unsafe { Encoding::from_raw(e) })
}

/// `rb_enc_find_index` + `rb_enc_from_index`: may autoload the encoding,
/// which can raise (`LoadError`) and is a GC point; `None` for a name Ruby
/// does not know. Not `rb_enc_find`, which answers ASCII-8BIT for one.
#[inline]
pub fn enc_find(_: &Ruby, name: &core::ffi::CStr) -> Result<Option<Encoding>, Error> {
    let p = name.as_ptr();
    let e = call(|| unsafe {
        let index = rb_sys::rb_enc_find_index(p);
        if index < 0 {
            core::ptr::null_mut()
        } else {
            rb_sys::rb_enc_from_index(index)
        }
    })?;
    // SAFETY: a non-null answer is one of Ruby's encodings, which live for
    // the process.
    Ok((!e.is_null()).then(|| unsafe { Encoding::from_raw(e) }))
}

/* ---- Arrays, Hashes, numbers (`bridge::build` spells these for the glue) ---- */

/// `rb_ary_new_capa`: raises `NoMemoryError`.
#[inline]
pub fn ary_new_capa(_: &Ruby, capa: usize) -> Result<RArray, Error> {
    let n = c_long::try_from(capa).unwrap_or(c_long::MAX);
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_new_capa(n)
    })?;
    typed(v, RArray::from_value, "a C call made no Array")
}

/// `rb_ary_push`: raises `NoMemoryError`, and `FrozenError` for a frozen
/// Array.
#[inline]
pub fn ary_push(a: RArray, v: Value) -> Result<(), Error> {
    let (a, v) = (a.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_push(a, v)
    })
    .map(|_| ())
}

/// `rb_ivar_set` for one of Makiri's hidden ivars (a name without `@`, so
/// Ruby code cannot see it): raises `FrozenError` for a frozen object and
/// `NoMemoryError`.
#[inline]
pub fn ivar_set(obj: Value, id: ID, v: Value) -> Result<(), Error> {
    let (o, v) = (obj.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ivar_set(o, id, v)
    })
    .map(|_| ())
}

/// `rb_attr_get`: a hidden ivar's value, or nil. Raises nothing and runs no
/// Ruby code (an attribute read, not a method call).
#[inline]
pub fn ivar_get(obj: Value, id: ID) -> Value {
    // SAFETY: a live object; the read raises nothing.
    unsafe { Value::from_raw(rb_sys::rb_attr_get(obj.as_raw(), id)) }
}

/// A Fixnum for a table key: `None` past a Fixnum.
///
/// Built from its bits - `(n << 1) | FIXNUM_FLAG`, what `long2fix` does -
/// rather than through a C `long`: a Fixnum is as wide as a VALUE, less one
/// bit, while a `long` is 32 bits on Windows, where a node token past 2^32
/// was refused as past a Fixnum.
#[inline]
fn fixnum_key(n: usize) -> Option<VALUE> {
    /* The largest positive Fixnum: a signed VALUE less its flag bit. */
    const FIXNUM_MAX: VALUE = VALUE::MAX >> 2;
    let v = VALUE::try_from(n).ok().filter(|&v| v <= FIXNUM_MAX)?;
    Some((v << 1) | rb_sys::FIXNUM_FLAG as VALUE)
}

/// `rb_hash_lookup2` at the Fixnum `n`: a Fixnum hashes and compares by
/// value in C, so no Ruby code runs; protected all the same, and a raise
/// passed on, as [`hash_lookup`]'s is.
#[inline]
pub fn table_get(h: RHash, n: usize) -> Result<Option<Value>, Error> {
    const UNDEF: VALUE = rb_sys::Qundef as VALUE;
    let Some(key) = fixnum_key(n) else {
        return Ok(None);
    };
    let h = h.as_raw();
    let v = call(|| unsafe { rb_sys::rb_hash_lookup2(h, key, UNDEF) })?;
    // SAFETY: a value of the Hash's, or `Qundef`, which is not handed out.
    Ok((v != UNDEF).then(|| unsafe { Value::from_raw(v) }))
}

/// `rb_hash_aset` at the Fixnum `n`: raises `NoMemoryError`.
#[inline]
pub fn table_set(h: RHash, n: usize, v: Value) -> Result<(), Error> {
    let key = fixnum_key(n)
        .ok_or_else(|| crate::nupur::helpers::internal_error("a node token past a Fixnum"))?;
    let (h, v) = (h.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, key, v)
    })
    .map(|_| ())
}

/// `rb_hash_new`: raises `NoMemoryError`.
#[inline]
pub fn hash_new(_: &Ruby) -> Result<RHash, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_new()
    })?;
    typed(v, RHash::from_value, "a C call made no Hash")
}

/// A Hash no Ruby code can find: `rb_hash_new`, then `rb_obj_hide` with
/// nothing between, so `ObjectSpace` never lists it - an object's private
/// table (a Document's node table), which is never shown. Raises
/// `NoMemoryError`. Hand it to no Ruby code: a hidden object has no class.
pub fn hidden_hash_new(ruby: &Ruby) -> Result<RHash, Error> {
    let h = hash_new(ruby)?;
    // SAFETY: the Hash just made, referenced only from this frame; hiding
    // writes only its class field.
    unsafe { rb_sys::rb_obj_hide(h.as_raw()) };
    Ok(h)
}

/// `rb_hash_aset` for a String or Symbol key, whose hash Ruby computes itself:
/// any other key would have its `#hash` and `#eql?` called, so it is a
/// `TypeError` here. Raises `NoMemoryError`, and `FrozenError` for a frozen
/// Hash.
#[inline]
pub fn hash_aset(h: RHash, k: Value, v: Value) -> Result<(), Error> {
    if RString::from_value(k).is_none() && Symbol::from_value(k).is_none() {
        return Err(crate::nupur::helpers::type_error(
            "a Hash key must be a String or a Symbol",
        ));
    }
    let (h, k, v) = (h.as_raw(), k.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, k, v)
    })
    .map(|_| ())
}

/// `rb_ll2inum`: raises `NoMemoryError` for a value past a Fixnum (a Bignum).
#[inline]
pub fn int_from_i64(_: &Ruby, n: i64) -> Result<Integer, Error> {
    use rb_sys::stable_api::{get_default, StableApiDefinition};
    /* A Fixnum is the value's own bits: no allocation, nothing to raise, no
     * protected call - the common case (a length, an index, a count). */
    /* A `long` is 32 bits on Windows, so the round trip is the range check. */
    let api = get_default();
    let l = n as c_long;
    if l as i64 == n && api.fixable(l) {
        // SAFETY: an immediate.
        return Ok(Integer::from_known(unsafe {
            Value::from_raw(api.long2fix(l))
        }));
    }
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ll2inum(n)
    })?;
    typed(v, Integer::from_value, "a C call made no Integer")
}

/// `rb_ull2inum`: as [`int_from_i64`].
#[inline]
pub fn int_from_u64(_: &Ruby, n: u64) -> Result<Integer, Error> {
    use rb_sys::stable_api::{get_default, StableApiDefinition};
    /* As `int_from_i64`. */
    let api = get_default();
    let l = n as core::ffi::c_ulong;
    if l as u64 == n && api.posfixable(l) {
        // SAFETY: an immediate; `posfixable` keeps it within a long.
        return Ok(Integer::from_known(unsafe {
            Value::from_raw(api.long2fix(l as c_long))
        }));
    }
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ull2inum(n)
    })?;
    typed(v, Integer::from_value, "a C call made no Integer")
}

/// `rb_float_new`: raises `NoMemoryError` for a value that is not a flonum.
#[inline]
pub fn float_new(_: &Ruby, f: f64) -> Result<Float, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_float_new(f)
    })?;
    typed(v, Float::from_value, "a C call made no Float")
}

/// `rb_enumeratorize_with_size` with no arguments and no size: raises
/// `NoMemoryError`.
#[inline]
pub fn enumeratorize(obj: Value, meth: Symbol) -> Result<Value, Error> {
    let (o, m) = (obj.as_raw(), meth.as_raw());
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_enumeratorize_with_size(o, m, 0, core::ptr::null(), None)
    })?;
    typed(v, Some, "a C call made no value")
}

/* ---- typed data and Ruby's allocator ---- */

/// `rb_check_typeddata`: raises `TypeError` for a value that is not of `ty`.
#[inline]
pub unsafe fn check_typeddata(v: VALUE, ty: *const rb_data_type_t) -> Result<*mut c_void, Error> {
    call(|| unsafe { rb_sys::rb_check_typeddata(v, ty) })
}

/// `ruby_xrealloc2`: raises `NoMemoryError`, and `ArgumentError` when
/// `n * size` overflows.
#[inline]
pub unsafe fn xrealloc2(p: *mut c_void, n: usize, size: usize) -> Result<*mut c_void, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::ruby_xrealloc2(p, n as rb_sys::size_t, size as rb_sys::size_t)
    })
}

/// `rb_data_typed_object_zalloc`: raises `NoMemoryError`. For an embeddable
/// type small enough, the zeroed data is the object's own slot.
#[inline]
pub unsafe fn typed_object_zalloc(
    klass: VALUE,
    size: usize,
    ty: *const rb_data_type_t,
) -> Result<VALUE, Error> {
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_data_typed_object_zalloc(klass, size as rb_sys::size_t, ty)
    })
}
