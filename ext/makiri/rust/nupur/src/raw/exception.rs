//! Exceptions: making one, the load check that a raise is told from a
//! `throw`, and a value's class name for a message.

use super::*;

/// An exception of `class` with the UTF-8 message `msg`: `rb_exc_new_str`,
/// which allocates and runs the class's `initialize`, and so raises.
#[inline]
pub fn exception_new(ruby: &Ruby, class: ExceptionClass, msg: &[u8]) -> Result<Value, Error> {
    let s = utf8_str_new(ruby, msg)?.as_sys();
    let class = class.as_sys();
    let exc = call(|inside| unsafe {
        injected_raise(inside);
        nupur_sys::raising::rb_exc_new_str(inside, class, s)
    })?;
    Ok(Value::from_sys(exc))
}

/// Does [`call`] still tell a raise from a `throw` or a `break`? It does by
/// `rb_protect`'s state, `TAG_RAISE`, a number Ruby does not document
/// (`protect`'s own doc), so the extension asks once as it loads: a raise is
/// made under `call` and must come back as an exception. `Ok(false)` is a Ruby
/// whose numbering this was not written for - the exit it left pending is
/// forgotten here, and the caller refuses to load.
pub fn raise_is_recognised(ruby: &Ruby) -> Result<bool, Error> {
    let exc = exception_new(ruby, ruby.exception_runtime_error(), b"raise self-check")?.as_sys();
    let outcome = call::<(), _>(|inside| unsafe { nupur_sys::raising::rb_exc_raise(inside, exc) });
    let recognised = matches!(
        &outcome,
        Err(e) if e.is_kind_of(ruby, crate::rb::Builtin::RuntimeError)
    );
    if !recognised {
        reset_pending_exit();
        forget_errinfo(ruby);
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
    let v = v.as_sys();
    let p = call(|inside| unsafe {
        injected_raise(inside);
        nupur_sys::raising::rb_obj_classname(inside, v)
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
