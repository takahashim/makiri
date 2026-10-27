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

/// Does [`call`] still tell a raise from a `throw` or a `break`? It does by
/// `rb_protect`'s state, `TAG_RAISE`, a number Ruby does not document
/// (`protect`'s own doc), so the extension asks once as it loads: a raise is
/// made under `call` and must come back as an exception. `Ok(false)` is a Ruby
/// whose numbering this was not written for - the exit it left pending is
/// forgotten here, and the caller refuses to load.
pub fn raise_is_recognised(ruby: &Ruby) -> Result<bool, Error> {
    let exc = exception_new(ruby, ruby.exception_runtime_error(), b"raise self-check")?.as_raw();
    let outcome = call::<(), _>(|| unsafe { rb_sys::rb_exc_raise(exc) });
    let recognised = matches!(
        &outcome,
        Err(e) if e.is_kind_of(ruby, crate::rb::Builtin::RuntimeError)
    );
    if !recognised {
        reset_pending_exit();
    }
    Ok(recognised)
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

/* ---- the rest, by what it wraps ---- */

mod check;
mod data;
mod define;
mod iterate;
mod string;
mod symbol;
mod values;
pub use check::*;
pub(crate) use data::*;
pub use define::*;
pub use iterate::*;
pub use string::*;
pub use symbol::*;
pub use values::*;
