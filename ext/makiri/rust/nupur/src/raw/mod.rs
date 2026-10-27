//! The Ruby C calls that can RAISE - every `longjmp` this extension can start -
//! and each one made under `rb_protect`, so the raise comes back as `Err`.
//!
//! A raise is `longjmp`, which crosses Rust frames without running their
//! destructors, and crossing a frame that has one (or a `catch_unwind`) is
//! undefined behaviour. So no raising call is made anywhere but here, and here
//! every one runs inside [`call`] ([`protect`]): a bare `rb_protect` whose `extern "C"`
//! thunk holds nothing but that one C call and the `Copy` values it reads. The
//! only frame a raise crosses is the thunk's own, which owns nothing. A
//! caller gets a `Result` and owns whatever it likes across the call.
//! `rake unsafe:boundaries` pins that these C names appear nowhere else in the
//! crate, so a new raising call is a new wrapper here, with its raise stated.
//!
//! The value-level wrappers are SAFE: they take and return `rb`'s
//! typed values (`RString`, `RArray`, `RHash`, `Value`, ...), slices and the
//! layer's `Encoding`, so a live value of the right kind is the type's
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
//! What is here is checked, not kept by hand: which C functions raise is
//! one table in the lints (`makiri_lints`' `NON_RAISING`, every function
//! not in it taken to raise), and `rake dylint` fails on a call of Ruby's C
//! in this module outside [`call`] - a read that raises nothing belongs with
//! the values (`rb::value`, `rb::method` for the current frame) - and on a
//! `call` that protects nothing. The protection itself, `$!` and the calls
//! that raise on purpose are [`protect`]'s, the one part held to neither.

#![allow(unsafe_code)]
/* Every wrapper's contract is the module's, stated once above. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use crate::rb::value::AsRawValue;
use crate::rb::{
    Error, ExceptionClass, Float, Id, Integer, RArray, RClass, RHash, RModule, RString, Range,
    Ruby, Symbol, Value,
};
use rb_sys::{rb_data_type_t, VALUE};

use crate::encoding::Encoding;

mod protect;
#[cfg(feature = "alloc-inject")]
pub use protect::set_raise_injector;
use protect::{call, injected_raise};
pub(crate) use protect::{
    exc_fatal, exc_raise, keeping_errinfo, pending_exit, reset_pending_exit, resume,
};

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
    of(unsafe { Value::from_raw(v) }).ok_or_else(|| crate::rb::error::internal_error(what))
}

/// A length for the C API's `long` (whichever width the call's parameter
/// has); `ArgumentError` for one it cannot hold - past 2^31 on Windows.
#[inline]
fn c_len<L: TryFrom<usize>>(n: usize) -> Result<L, Error> {
    L::try_from(n).map_err(|_| crate::rb::error::arg_error("string too large"))
}

/// An exception of `class` with the UTF-8 message `msg`: `rb_exc_new_str`,
/// which allocates and runs the class's `initialize`, and so raises.
#[inline]
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

/// The longest class name [`with_classname`] hands on; a longer one is cut.
const CLASSNAME_MAX: usize = 128;

/// `rb_obj_classname`: the name of `v`'s class, handed to `f` as bytes. Raises
/// `NoMemoryError`: an anonymous class's name (`#<Class:0x...>`) is a String
/// made on each call, referenced by nothing once the call returns - so `f`
/// is handed a copy, up to [`CLASSNAME_MAX`] bytes, and may do anything.
pub fn with_classname<R>(v: Value, f: impl FnOnce(&[u8]) -> R) -> Result<R, Error> {
    let v = v.as_raw();
    let p = call(|| unsafe {
        injected_raise();
        rb_sys::rb_obj_classname(v)
    })?;
    if p.is_null() {
        return Ok(f(b"Object"));
    }
    /* Copied out at once, before `f` runs: the name may be a temporary
     * String nothing references, which a collection `f` set off could free.
     * A name past the buffer is cut - it is written into a message. */
    let mut buf = [0u8; CLASSNAME_MAX];
    // SAFETY: a NUL-terminated C string owned by the class or by that
    // temporary String, read and copied here with nothing run between.
    let name = unsafe { core::ffi::CStr::from_ptr(p) }.to_bytes();
    /* At a character boundary, so a cut name ends in a whole character. */
    let n = crate::msg::floor_char_boundary(name, CLASSNAME_MAX);
    if let (Some(d), Some(s)) = (buf.get_mut(..n), name.get(..n)) {
        d.copy_from_slice(s);
    }
    Ok(f(buf.get(..n).unwrap_or_default()))
}

/// `rb_hash_lookup2` at `key`, `Ok(None)` for no entry. The key must be a
/// Symbol or a plain String, hashed and compared in C - any other is a
/// `TypeError` here, as [`hash_aset`]'s, since its own `#hash` would run - but
/// a key already IN the Hash may
/// be any object, and one whose hash collides is compared with its `eql?`,
/// the caller's Ruby, which can raise or `throw`. That comes back as `Err`,
/// to be passed on: read as "no entry", the exception would be lost and the
/// keyword taken as absent. In the raise sweep's set (`injected_raise`), so
/// that path is run.
pub fn hash_lookup(h: RHash, key: Value) -> Result<Option<Value>, Error> {
    const UNDEF: VALUE = rb_sys::Qundef as VALUE;
    plain_key_or_refuse(key)?;
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
/// none, and an autoload runs Ruby code - for a class the extension's own
/// Ruby defines.
pub fn const_get(ruby: &Ruby, outer: Value, name: &core::ffi::CStr) -> Result<Value, Error> {
    let id = intern_cstr(ruby, name)?.as_raw();
    let outer = outer.as_raw();
    let v = call(|| unsafe { rb_sys::rb_const_get(outer, id) })?;
    // SAFETY: the constant's value, live while it is bound.
    Ok(unsafe { Value::from_raw(v) })
}

/// `recv.name(*args)` (`rb_funcallv`): the caller's Ruby, which is why each
/// call of it is allowed where it is made (`rake dylint`'s
/// `runs_callers_ruby`) - the one use is
/// a `Data` class of the extension's own, whose `new` takes the
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
    let id = intern_cstr(ruby, name)?.as_raw();
    let argc = c_int::try_from(args.len())
        .map_err(|_| crate::rb::error::arg_error("too many arguments"))?;
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

/// `rb_big2ll`: raises `RangeError` past an `i64`. For a Bignum only - the C
/// reads a Bignum's header unchecked - so crate-private: the one caller is
/// `Integer::to_i64`, which answers a Fixnum itself.
pub(crate) fn big2ll(n: Integer) -> Result<i64, Error> {
    let n = n.as_raw();
    Ok(i64::from(call(|| unsafe { rb_sys::rb_big2ll(n) })?))
}

/// `rb_big2ull`: raises `RangeError` past a `u64`. For a non-negative Bignum
/// only (a negative one would wrap), so crate-private: the one caller is
/// `Integer::to_u64`, which refuses a negative value and answers a Fixnum
/// itself.
pub(crate) fn big2ull(n: Integer) -> Result<u64, Error> {
    let n = n.as_raw();
    Ok(u64::from(call(|| unsafe { rb_sys::rb_big2ull(n) })?))
}

/* ---- definitions, at `Init_` ---- */

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
pub fn define_module(_: &Ruby, name: &core::ffi::CStr) -> Result<Value, Error> {
    let name = name.as_ptr();
    let v = call(|| unsafe { rb_sys::rb_define_module(name) })?;
    // SAFETY: as `define_class_under`.
    Ok(unsafe { Value::from_raw(v) })
}

/// `rb_const_set`: raises `FrozenError` on a frozen namespace.
pub fn const_set(ruby: &Ruby, outer: Value, name: &core::ffi::CStr, v: Value) -> Result<(), Error> {
    let id = intern_cstr(ruby, name)?.as_raw();
    let (outer, v) = (outer.as_raw(), v.as_raw());
    call(|| unsafe { rb_sys::rb_const_set(outer, id, v) })
}

/// `rb_include_module`: raises `ArgumentError` for a cyclic include,
/// `FrozenError` on a frozen class.
pub fn include_module(klass: Value, module: Value) -> Result<(), Error> {
    let (klass, module) = (klass.as_raw(), module.as_raw());
    call(|| unsafe { rb_sys::rb_include_module(klass, module) })
}

/// `rb_undef_alloc_func`: raises `TypeError` for a singleton class, which
/// may not have an allocator of its own taken.
pub fn undef_alloc_func(klass: RClass) -> Result<(), Error> {
    let k = klass.as_raw();
    call(|| unsafe { rb_sys::rb_undef_alloc_func(k) })
}

/// `rb_obj_freeze`: raises `NoMemoryError` - freezing moves an object with
/// instance variables to a frozen shape, which may allocate.
pub fn obj_freeze(v: Value) -> Result<(), Error> {
    let v = v.as_raw();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_obj_freeze(v)
    })
    .map(|_| ())
}

/// `rb_obj_reveal`: show `obj`, hidden by `rb_obj_hide`, as an instance of
/// `klass`. Raises `NoMemoryError`: the class is written with a write
/// barrier, which may grow the remembered set (found by the C API raise
/// survey, takahashim.github.io/ruby-c-apis - Ruby's header says nothing of
/// it). On `Err` the object stays hidden.
pub fn obj_reveal(obj: Value, klass: Value) -> Result<(), Error> {
    let (o, k) = (obj.as_raw(), klass.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_obj_reveal(o, k)
    })
    .map(|_| ())
}

/// `rb_define_method` / `_private_method` / `_singleton_method`: raise
/// `FrozenError` on a frozen target.
pub(crate) fn define_method(
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

/* ---- blocks and Hash walks ---- */

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
        latch: crate::caught::PanicLatch,
    }
    unsafe extern "C" fn each(k: VALUE, v: VALUE, arg: VALUE) -> c_int {
        const CONTINUE: c_int = rb_sys::st_retval::ST_CONTINUE as c_int;
        const STOP: c_int = rb_sys::st_retval::ST_STOP as c_int;
        crate::no_unwind::must_not_unwind::<crate::no_unwind::HashEach, _>(|| {
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
        latch: crate::caught::PanicLatch::new(),
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

/// `rb_check_frozen` returning its `FrozenError` rather than raising it.
///
/// The error is Ruby's own - the message naming the receiver and `#receiver`
/// set - because the check still runs, protected. An unfrozen value, every
/// mutator's normal case, makes no call.
#[inline]
pub fn check_frozen(v: Value) -> Result<(), Error> {
    if !crate::rb::ReprValue::is_frozen(v) {
        return Ok(());
    }
    let v = v.as_raw();
    call(|| unsafe { rb_sys::rb_check_frozen(v) })
}

/// `rb_range_beg_len` with `err = 0` over a collection of `count` elements:
/// the start and the length it selects, `None` for a start outside it, and
/// `RangeError` for a bound too large for a `long`. A Range whose ends are
/// not Integers or nil is a `TypeError`, checked here: the C would convert
/// such an end with its `to_int`.
#[inline]
pub fn range_beg_len(range: Value, count: usize) -> Result<Option<(usize, usize)>, Error> {
    let Some(r) = crate::rb::Range::from_value(range) else {
        return Err(crate::rb::error::type_error(
            "a Range whose ends are Integers (or nil) was expected",
        ));
    };
    /* Checked here, not left to the caller: an end of any other kind would
     * have its `to_int` run by the C, the caller's Ruby. */
    let (beg, end, _) = range_values(r)?;
    if [beg, end]
        .iter()
        .any(|&e| !crate::rb::ReprValue::is_nil(e) && Integer::from_value(e).is_none())
    {
        return Err(crate::rb::error::type_error(
            "a Range whose ends are Integers (or nil) was expected",
        ));
    }
    let count = count
        .try_into()
        .map_err(|_| crate::rb::error::arg_error("a collection too large for a long"))?;
    let (mut beg, mut len) = (0, 0);
    let (b, l) = (core::ptr::addr_of_mut!(beg), core::ptr::addr_of_mut!(len));
    let r = range.as_raw();
    let answer = call(|| unsafe { rb_sys::rb_range_beg_len(r, b, l, count, 0) })?;
    if answer != rb_sys::Qtrue as VALUE {
        return Ok(None);
    }
    /* Within `0..=count` both, when the C answers true. */
    match (usize::try_from(beg), usize::try_from(len)) {
        (Ok(beg), Ok(len)) => Ok(Some((beg, len))),
        _ => Err(crate::rb::error::internal_error(
            "rb_range_beg_len answered a negative bound",
        )),
    }
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
pub fn intern(_: &Ruby, name: &str) -> Result<Id, Error> {
    let (p, n) = (name.as_ptr() as *const c_char, c_len(name.len())?);
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern3(p, n, rb_sys::rb_utf8_encoding())
    })
    .map(Id::from_raw)
}

/// `rb_intern` of a NUL-terminated name: as [`intern`].
#[inline]
pub fn intern_cstr(_: &Ruby, name: &core::ffi::CStr) -> Result<Id, Error> {
    let p = name.as_ptr();
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_intern(p)
    })
    .map(Id::from_raw)
}

/// The Symbol of the UTF-8 `name`: [`intern`] and `rb_id2sym`, which raises
/// `NoMemoryError` when a dynamic Symbol is made.
#[inline]
pub(crate) fn sym(ruby: &Ruby, name: &str) -> Result<Symbol, Error> {
    let id = intern(ruby, name)?.as_raw();
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
pub(crate) fn utf8_str_new(_: &Ruby, bytes: &[u8]) -> Result<RString, Error> {
    let (p, n) = (bytes.as_ptr() as *const c_char, c_len(bytes.len())?);
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_utf8_str_new(p, n)
    })?;
    typed(v, RString::from_value, "a C call made no String")
}

/// A UTF-8 String of `len` bytes whose content is NOT written:
/// `rb_utf8_str_new(NULL, len)`, which raises `NoMemoryError`. What
/// `string::utf8_str_filled` writes in place.
///
/// # Safety
/// Before Ruby code can see the String, every one of its `len` bytes is
/// written or its length set to 0: its buffer holds whatever the allocation
/// held.
#[inline]
pub(crate) unsafe fn utf8_str_unwritten(_: &Ruby, len: usize) -> Result<RString, Error> {
    let n = c_len(len)?;
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_utf8_str_new(core::ptr::null(), n)
    })?;
    typed(v, RString::from_value, "a C call made no String")
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
    use crate::rb::ReprValue;
    if !v.is_kind_of(ruby.class_encoding()) && RString::from_value(v).is_none() {
        return Err(crate::rb::error::type_error(
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

/* ---- Arrays, Hashes, numbers (`build` spells these for a caller) ---- */

/// `rb_ary_new_capa`: raises `NoMemoryError`.
#[inline]
pub(crate) fn ary_new_capa(_: &Ruby, capa: usize) -> Result<RArray, Error> {
    let n = crate::int::capacity(capa);
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_new_capa(n)
    })?;
    typed(v, RArray::from_value, "a C call made no Array")
}

/// `rb_ary_push`: raises `NoMemoryError`, and `FrozenError` for a frozen
/// Array.
#[inline]
pub(crate) fn ary_push(a: RArray, v: Value) -> Result<(), Error> {
    let (a, v) = (a.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ary_push(a, v)
    })
    .map(|_| ())
}

/// `rb_ivar_set` for a hidden ivar (a name without `@`, so Ruby code cannot
/// see it): raises `FrozenError` for a frozen object and
/// `NoMemoryError`.
#[inline]
pub fn ivar_set(obj: Value, id: Id, v: Value) -> Result<(), Error> {
    let (o, v, id) = (obj.as_raw(), v.as_raw(), id.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_ivar_set(o, id, v)
    })
    .map(|_| ())
}

/// `rb_hash_lookup2` at the Fixnum `n`: a Fixnum hashes and compares by
/// value in C, so no Ruby code runs; protected all the same, and a raise
/// passed on, as [`hash_lookup`]'s is.
#[inline]
pub(crate) fn table_get(h: RHash, n: u32) -> Result<Option<Value>, Error> {
    const UNDEF: VALUE = rb_sys::Qundef as VALUE;
    /* No entry has a key past a Fixnum - past 2^30 - 1 on Windows. */
    let Some(key) = crate::int::fixnum(i64::from(n)) else {
        return Ok(None);
    };
    let h = h.as_raw();
    let v = call(|| unsafe { rb_sys::rb_hash_lookup2(h, key, UNDEF) })?;
    // SAFETY: a value of the Hash's, or `Qundef`, which is not handed out.
    Ok((v != UNDEF).then(|| unsafe { Value::from_raw(v) }))
}

/// `rb_hash_aset` at the Fixnum `n`: raises `NoMemoryError`.
#[inline]
pub(crate) fn table_set(h: RHash, n: u32, v: Value) -> Result<(), Error> {
    /* No entry for a key past a Fixnum: refused, never wrapped round. */
    let key = crate::int::fixnum(i64::from(n)).ok_or_else(|| {
        Error::new(
            crate::rb::Builtin::RangeError,
            "a table key past a Fixnum (more than 2^30 nodes on Windows)",
        )
    })?;
    let (h, v) = (h.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, key, v)
    })
    .map(|_| ())
}

/// A key whose hash Ruby computes itself ([`crate::rb::value::plain_key`]),
/// or the `TypeError` for one whose `#hash` and `#eql?` would be called.
#[inline]
fn plain_key_or_refuse(k: Value) -> Result<(), Error> {
    if crate::rb::value::plain_key(k) {
        return Ok(());
    }
    Err(crate::rb::error::type_error(
        "a Hash key must be a Symbol or a String (not a subclass's)",
    ))
}

/// `rb_hash_new`: raises `NoMemoryError`.
#[inline]
pub(crate) fn hash_new(_: &Ruby) -> Result<RHash, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_new()
    })?;
    typed(v, RHash::from_value, "a C call made no Hash")
}

/// A Hash no Ruby code can find: `rb_hash_new`, then `rb_obj_hide` with
/// nothing between, so `ObjectSpace` never lists it - an object's private
/// table ([`crate::table::IntTable`]), which is never shown. Raises
/// `NoMemoryError`. Hand it to no Ruby code: a hidden object has no class.
pub(crate) fn hidden_hash_new(ruby: &Ruby) -> Result<RHash, Error> {
    let h = hash_new(ruby)?;
    // SAFETY: the Hash just made, referenced only from this frame.
    unsafe { crate::rb::value::obj_hide(crate::rb::ReprValue::as_value(h)) };
    Ok(h)
}

/// `rb_hash_aset` for a Symbol or String key, whose hash Ruby computes
/// itself ([`crate::rb::value::plain_key`]): any other key - an instance of a
/// String subclass included - would have its `#hash` and `#eql?` called, so
/// it is a `TypeError` here. Raises `NoMemoryError`, and `FrozenError` for a
/// frozen Hash.
#[inline]
pub(crate) fn hash_aset(h: RHash, k: Value, v: Value) -> Result<(), Error> {
    plain_key_or_refuse(k)?;
    let (h, k, v) = (h.as_raw(), k.as_raw(), v.as_raw());
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_hash_aset(h, k, v)
    })
    .map(|_| ())
}

/// `rb_ll2inum`: raises `NoMemoryError` for a value past a Fixnum (a Bignum).
#[inline]
pub(crate) fn int_from_i64(_: &Ruby, n: i64) -> Result<Integer, Error> {
    /* A Fixnum is the value's own bits: no allocation, nothing to raise, no
     * protected call - the common case (a length, an index, a count). */
    if let Some(v) = crate::int::fixnum(n) {
        // SAFETY: an immediate.
        return Ok(Integer::from_known(unsafe { Value::from_raw(v) }));
    }
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ll2inum(n)
    })?;
    typed(v, Integer::from_value, "a C call made no Integer")
}

/// `rb_ull2inum`: as [`int_from_i64`].
#[inline]
pub(crate) fn int_from_u64(_: &Ruby, n: u64) -> Result<Integer, Error> {
    /* As `int_from_i64`. */
    if let Some(v) = crate::int::posfixnum(n) {
        // SAFETY: an immediate.
        return Ok(Integer::from_known(unsafe { Value::from_raw(v) }));
    }
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_ull2inum(n)
    })?;
    typed(v, Integer::from_value, "a C call made no Integer")
}

/// `rb_float_new`: raises `NoMemoryError` for a value that is not a flonum.
#[inline]
pub(crate) fn float_new(_: &Ruby, f: f64) -> Result<Float, Error> {
    let v = call(|| unsafe {
        injected_raise();
        rb_sys::rb_float_new(f)
    })?;
    typed(v, Float::from_value, "a C call made no Float")
}

/// `rb_enumeratorize_with_size` with no arguments and no size: raises
/// `NoMemoryError`.
#[inline]
pub(crate) fn enumeratorize(obj: Value, meth: Symbol) -> Result<Value, Error> {
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
pub(crate) unsafe fn check_typeddata(
    v: VALUE,
    ty: *const rb_data_type_t,
) -> Result<*mut c_void, Error> {
    call(|| unsafe { rb_sys::rb_check_typeddata(v, ty) })
}

/// `rb_data_typed_object_zalloc`: raises `NoMemoryError`. For an embeddable
/// type small enough, the zeroed data is the object's own slot.
#[inline]
pub(crate) unsafe fn typed_object_zalloc(
    klass: VALUE,
    size: usize,
    ty: *const rb_data_type_t,
) -> Result<VALUE, Error> {
    let size = rb_sys::size_t::try_from(size)
        .map_err(|_| crate::rb::error::internal_error("a wrapped object too large"))?;
    call(|| unsafe {
        injected_raise();
        rb_sys::rb_data_typed_object_zalloc(klass, size, ty)
    })
}
