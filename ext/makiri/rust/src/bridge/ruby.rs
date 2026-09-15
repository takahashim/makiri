//! The raising Ruby C API, contained.
//!
//! `rb_raise` and every C function that can raise unwind with `longjmp`, which
//! skips Rust destructors in every frame it crosses (see `glue/mod.rs`). The
//! functions here are the places the extension still calls such a function,
//! and each turns the raise into a [`magnus::Error`] instead: the caller hands
//! it back as `Err`, and magnus raises only after the Rust frames have returned
//! normally. [`raise`] is the one exit for an entry point Ruby calls with the C
//! convention, which has no `Result` to return.

use core::ffi::{c_int, c_void};

use magnus::error::ErrorType;
use magnus::rb_sys::{protect, AsRawValue, FromRawValue};
use magnus::{Error, RString, Value};
use rb_sys::{rb_data_type_t, VALUE};

/// `v` as a String, coerced the way `rb_String` does (`to_str`, else `to_s`).
///
/// A String passes straight through, so the common case is one type check.
/// Anything else runs its conversion under `protect`: a `to_s` that raises
/// comes back as `Err` rather than unwinding through the caller.
///
/// # Safety
/// Under the GVL, with `v` a live VALUE.
pub unsafe fn string_of(v: VALUE) -> Result<VALUE, Error> {
    if RString::from_value(Value::from_raw(v)).is_some() {
        return Ok(v);
    }
    protect(|| rb_sys::rb_String(v))
}

/// The data pointer of a TypedData object of type `ty` (or a type deriving
/// from it), or the `TypeError` Ruby's own check raises.
///
/// The type is tested first, so a well-typed object - every call on the normal
/// path - never enters `protect`. Only a mismatch runs `rb_check_typeddata`
/// under it, which is what keeps the error message Ruby's own, word for word,
/// on every supported Ruby.
///
/// # Safety
/// Under the GVL, with `v` a live VALUE and `ty` a registered data type.
pub unsafe fn typed_data(v: VALUE, ty: *const rb_data_type_t) -> Result<*mut c_void, Error> {
    if rb_sys::rb_typeddata_is_kind_of(v, ty) != 0 {
        return Ok(rb_sys::rb_check_typeddata(v, ty));
    }
    match protect(|| rb_sys::rb_check_typeddata(v, ty) as VALUE) {
        Err(e) => Err(e),
        /* rb_typeddata_is_kind_of said no, so the check should have raised. */
        Ok(_) => Err(Error::new(
            magnus::Ruby::get_unchecked().exception_type_error(),
            "wrong argument type",
        )),
    }
}

/// Raise `e` from an entry point Ruby calls with the C convention.
///
/// Only for the frame Ruby itself called: it unwinds with `longjmp`, so nothing
/// between Ruby and this call may own a resource. The error is turned into
/// Ruby objects and dropped before the jump, so it does not leak either.
///
/// # Safety
/// Under the GVL, from a frame whose callers own nothing that needs dropping.
pub unsafe fn raise(e: Error) -> ! {
    let jump = match e.error_type() {
        ErrorType::Exception(x) => Err(x.as_raw()),
        ErrorType::Error(class, msg) => {
            let s = rb_sys::rb_utf8_str_new(msg.as_ptr() as *const _, msg.len() as _);
            Err(rb_sys::rb_exc_new_str(class.as_raw(), s))
        }
        ErrorType::Jump(tag) => Ok(*tag as c_int),
    };
    drop(e);
    match jump {
        Err(exc) => rb_sys::rb_exc_raise(exc),
        Ok(tag) => rb_sys::rb_jump_tag(tag),
    }
}
