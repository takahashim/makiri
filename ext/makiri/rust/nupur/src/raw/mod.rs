//! The Ruby C calls that can RAISE - every `longjmp` this extension can start -
//! and each one made under `rb_protect`, so the raise comes back as `Err`.
//!
//! A raise is `longjmp`, which crosses Rust frames without running their
//! destructors, and crossing a frame that has one (or a `catch_unwind`) is
//! undefined behaviour. So no raising call is made anywhere but here, and here
//! every one runs inside [`call`] ([`protect`]): a bare `rb_protect` whose `extern "C"`
//! thunk holds nothing but that one C call and the `Copy` values it reads. The
//! only frame a raise crosses is the thunk's own, which owns nothing. A
//! caller gets a `Result` and owns whatever it likes across the call. The C
//! call is `nupur_sys::raising`'s, which takes the `&Protected` that only
//! `call` lends its closure.
//! `rake dylint`'s `raising_call_outside_protect` refuses a raising C call
//! anywhere else, so a new one is a new wrapper here, with its raise stated.
//!
//! The value-level wrappers are SAFE: they take and return `rb`'s
//! typed values (`RString`, `RArray`, `RHash`, `Value`, ...), slices and the
//! layer's `Encoding`, so a live value of the right kind is the type's
//! promise, not the caller's - under the rule every Ruby extension keeps, that
//! a Ruby value stays on the stack (`rake dylint`'s `ruby_value_on_heap`).
//! One that
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
//! the values (`rb::value`, `rb::object` for an object's ivars and class,
//! `rb::method` for the current frame) - and on a
//! `call` that protects nothing. The protection itself, `$!` and the calls
//! that raise on purpose are [`protect`]'s, the one part held to neither.

#![allow(unsafe_code)]
/* Every wrapper's contract is the module's, stated once above. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use crate::rb::value::AsSys;
use crate::rb::{
    Error, ExceptionClass, Float, Id, Integer, RArray, RClass, RHash, RModule, RString, Range,
    Ruby, Symbol, Value,
};
use nupur_sys::rb_data_type_t;

use crate::encoding::Encoding;

mod protect;
#[cfg(feature = "alloc-inject")]
pub(crate) use protect::set_raise_injector;
use protect::{call, injected_raise};
pub(crate) use protect::{
    exc_fatal, exc_raise, forget_errinfo, keeping_errinfo, pending_exit, reset_pending_exit, resume,
};

/* ---- what the value-level wrappers share ---- */

/// The live value a wrapped C call returned, as a `T` - checked to be one, so
/// a C function that answered with another kind is an error, not a value read
/// as the wrong type.
#[inline]
fn typed<T>(
    v: nupur_sys::value::Value,
    of: fn(Value) -> Option<T>,
    what: &'static str,
) -> Result<T, Error> {
    // SAFETY: `v` is what a Ruby C call just returned: a live value, on this
    // frame's stack until it is converted.
    /* `what` is the whole message, a literal: this runs on the path that
     * raises a caught panic, which must stay `fmt`-free (`rake no_unwind`). */
    of(Value::from_sys(v)).ok_or_else(|| crate::rb::error::internal_error(what))
}

/// A length for the C API's `long` (whichever width the call's parameter
/// has); `ArgumentError` for one it cannot hold - past 2^31 on Windows.
#[inline]
fn c_len<L: TryFrom<usize>>(n: usize) -> Result<L, Error> {
    L::try_from(n).map_err(|_| crate::rb::error::arg_error("string too large"))
}

/* ---- the rest, by what it wraps ---- */

mod check;
mod data;
mod define;
mod exception;
mod iterate;
mod string;
mod symbol;
mod values;
pub use check::*;
pub(crate) use data::*;
pub use define::*;
pub use exception::*;
pub use iterate::*;
pub use string::*;
pub use symbol::*;
pub use values::*;
