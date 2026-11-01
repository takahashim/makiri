//! How a raise becomes an `Err`, and how one is raised again, in Nupur's
//! types: the protection every wrapper of [`super`] runs its C call under
//! ([`call`]), the exit a protected call left pending, `$!`, and the calls
//! that leave on purpose ([`exc_raise`], [`resume`], [`exc_fatal`]).
//!
//! The protection itself is `nupur_sys::protect`'s - the `rb_protect` thunk,
//! the pending exit, `$!` - and each function here is its safe face: it holds
//! the proof of the GVL its caller has, and makes the raw exception a
//! `nupur_sys` call came back with an [`Error`].
//!
//! The rest of `raw` is the wrappers, every one a C call made inside
//! [`call`]; `rake dylint` holds it to that (`unraising_call_in_raw`).

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use crate::rb::value::AsRawValue;
use crate::rb::{Error, Ruby, Value};
use nupur_sys::protect::{Caught, Protected};

pub(crate) use nupur_sys::protect::reset_pending_exit;

pub(super) use nupur_sys::protect::injected_raise;
#[cfg(feature = "alloc-inject")]
pub(crate) use nupur_sys::protect::set_raise_injector;

/// The exit pending, as the `Err` that resumes it, or `None`.
#[inline]
pub(crate) fn pending_exit() -> Option<Error> {
    nupur_sys::protect::pending_exit().map(Error::jump)
}

/// Resume the non-local exit `state` stands for, which a protected call
/// caught: the one way a pending exit leaves the layer. Only `Error::raise`
/// calls it, as a trampoline's last act, with nothing owned.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "crate-private: only a trampoline's raise resumes an exit, with the GVL"
    )
)]
pub(crate) fn resume(state: c_int) -> ! {
    // SAFETY: a trampoline's last act, under the GVL, with nothing owned.
    unsafe { nupur_sys::protect::resume(state) }
}

/// Run `f` - one C call, and the `Copy` values it reads - under
/// `nupur_sys::protect::call`; a raise comes back as `Err`, and `$!` is
/// cleared (an error the caller handles rather than raises is run under
/// [`keeping_errinfo`]). `F: Copy` and `T: Copy` are that call's: the
/// thunk's frame owns nothing a skipped destructor would leak. `f` is lent
/// the `&Protected` that `nupur_sys::raising`'s wrappers take.
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "the raw module's alone: every wrapper that hands it its C call holds the proof"
    )
)]
pub(super) fn call<T: Copy, F: FnOnce(&Protected) -> T + Copy>(f: F) -> Result<T, Error> {
    // SAFETY: every caller is a wrapper of `raw` holding a proof of the GVL;
    // `f` owns nothing (it is `Copy`).
    match unsafe { nupur_sys::protect::call(f) } {
        Ok(out) => Ok(out),
        Err(caught) => Err(error_of(caught)),
    }
}

/// The `Err` a protected call's [`Caught`] stands for.
#[inline]
fn error_of(caught: Caught) -> Error {
    match caught {
        // SAFETY: the exception Ruby just left in `$!`, taken, on this
        // frame's stack until it is wrapped.
        Caught::Exception(exc) => Error::exception(unsafe { Value::from_raw(exc) }),
        Caught::Jump(state) => Error::jump(state),
        Caught::NotRun => crate::rb::error::internal_error("a protected call did not run"),
    }
}

/// Run `f`, protected calls whose error the caller handles rather than
/// raises, with `$!` put back as it was after
/// (`nupur_sys::protect::keeping_errinfo`).
#[inline]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        ffi_without_gvl_proof,
        reason = "crate-private: called only from functions holding a proof, around their protected calls"
    )
)]
pub(crate) fn keeping_errinfo<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: under the GVL, as every caller's proof says.
    unsafe { nupur_sys::protect::keeping_errinfo(f) }
}

/// Clear the error info a protected call left behind
/// (`nupur_sys::protect::forget_errinfo`).
#[inline]
pub(crate) fn forget_errinfo(_: &Ruby) {
    // SAFETY: the token is the GVL.
    unsafe { nupur_sys::protect::forget_errinfo() }
}

/// `rb_exc_raise`: raise `exc`. Raises, by design - only `Error::raise` calls
/// it, with nothing owned.
#[inline]
pub(crate) fn exc_raise(exc: Value) -> ! {
    // SAFETY: `exc` is a live exception object, a Ruby value and so the
    // proof of the GVL; the one caller owns nothing.
    unsafe { nupur_sys::protect::exc_raise(exc.as_raw()) }
}

/// `rb_exc_fatal`: raise `exc` as Ruby's `fatal` - for a panic in
/// the extension's `Init_`, where a half-defined extension must not be used.
/// Raises, by design.
#[inline]
pub(crate) fn exc_fatal(exc: Value) -> ! {
    // SAFETY: as `exc_raise`.
    unsafe { nupur_sys::protect::exc_fatal(exc.as_raw()) }
}
