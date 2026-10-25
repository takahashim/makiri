//! `nupur`: a Ruby binding layer for an extension - values, exceptions,
//! method trampolines, TypedData, the GVL, and every C call that can raise,
//! over rb-sys alone.
//!
//! It knows nothing of the extension it serves - a crate of its own, which
//! does not depend on the extension - and what it needs of the extension
//! (the exception a broken invariant raises, the raise sweep's hook) is
//! registered at `Init_` ([`rb::error::INTERNAL_ERROR`],
//! `raw::set_raise_injector`). Every `unsafe` the extension needs to talk to
//! Ruby is here, each with its contract stated. The crate denies
//! `unsafe_code`, and the files that need it carry an `allow` each.
//!
//! # What the layer guarantees
//!
//! - **No raise crosses Rust.** Every C call that can raise runs under
//!   `rb_protect` ([`raw`]) and comes back as `Err`; a method's `Err` is
//!   raised by its trampoline ([`rb::method::finish`]) once every Rust frame
//!   of the method has returned.
//! - **No panic reaches C.** A trampoline catches a panic and raises the
//!   internal error in its place; a callback C runs latches one
//!   ([`caught::PanicLatch`]) for its caller to resume. What must not unwind
//!   (an `extern "C"` function, a `Drop` the GC runs) is a [`no_unwind`]
//!   site, which a release build with `--cfg nupur_no_unwind` proves.
//! - **The GVL is a token.** A [`rb::Ruby`] is made only by a trampoline and
//!   by [`rb::method::run_init`], both `unsafe fn`s only the macros' expansion
//!   (and the extension's `Init_`) call; everything that calls Ruby takes one,
//!   or a Ruby value, which exists only where one did.
//! - **A wrapped object's values are its slots** ([`typed`]), marked by the
//!   layer; a payload holds none. Memory reported to the GC is given back by
//!   the free callback. One is made only of a [`rb::DataClass`], a class
//!   defined without an allocator, so Ruby cannot make one with no payload.
//! - **A String is read by copying** ([`string`]): no slice of one is handed
//!   out.
//! - **A C integer is converted in one place** ([`int`]): a `long` is 32
//!   bits on Windows, and the conversions are written once over either
//!   width and tested at both; everything else converts with `From` /
//!   `TryFrom` and names no `long`.
//! - **A non-local exit is resumed as it was.** While a `break` or `throw`
//!   a protected call caught is on its way to the trampoline, no protected
//!   call runs C, so the thread's error info Ruby resumes it from is left
//!   alone.
//!
//! # What the extension must uphold
//!
//! The types do not carry these; a crate using the layer keeps them, by
//! review or by lints (Makiri runs `lints/makiri_lints` on itself and on
//! this crate):
//!
//! - **A Ruby value stays where the GC's stack scan sees it.** A [`rb::Value`]
//!   (and every handle over one, [`typed::Typed`] included) is `Copy` and
//!   `!Send`: on the stack it is kept alive and in place, but in a `Vec`, a
//!   `Box`, a `static` or a payload's field it is not, and is freed or moved
//!   while still held. Keep such values on the stack, in a Ruby Array, in a
//!   wrapped object's slots, or in an [`rb::Opaque`] / [`rb::RbConst`].
//! - **A GVL-released body runs no Ruby.** [`gvl::without_gvl`]'s `Send`
//!   bound keeps a token and a value out of it, but not a call of a layer
//!   function that needs neither (making an [`rb::Error`], say) - harmless
//!   today, yet not a guarantee.
//! - **An `unsafe` call keeps its contract** - [`rb::method::finish`],
//!   [`rb::method::run_init`], [`rb::Ruby::get_unchecked`] - each stated where
//!   it is defined.

#![deny(unsafe_code)]
/* The casts that lose a value. Where the code is built a C `long` is `i64`
 * itself, so these see nothing there that `c_integer_outside_int` does not;
 * on Windows, whose `long` is 32 bits, clippy sees the real types - CI runs
 * it on that cell too. `int` is the one module that casts. */
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]
/* `i64::from(a_c_long)` is a no-op here and a widening on Windows: the
 * conversions are written for every target, which this lint cannot see. */
#![allow(
    clippy::useless_conversion,
    reason = "C's integer types are aliases whose width is the target's"
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
/* `rake dylint` sets `dylint_lib` for the whole build, the extension's run
 * compiling this crate as a plain dependency: there the extension's lints,
 * which the `allow`s here name, are unknown. */
#![cfg_attr(dylint_lib = "makiri_lints", allow(unknown_lints))]

/// The lints' self-test for this crate (`rake dylint`).
#[cfg(nupur_lint_probe)]
mod lint_probe;

/// The value and method layer: `Value` and the class-checked handles,
/// `Error`, the `method!` / `function!` trampolines, class definitions.
pub mod rb;

/// Every Ruby C call that can raise, each under `rb_protect`.
pub mod raw;

/// Every conversion between Rust's integer widths and C's `long` - the
/// layer's own: what it hands an extension is in Rust's widths.
pub(crate) mod int;

/// TypedData: wrapping a Rust value as a Ruby object, and its GC hooks.
pub mod typed;

/// Running a body with the GVL released.
pub mod gvl;

/// Building Ruby values: Arrays, Hashes, numbers, Symbols, UTF-8 Strings.
pub mod build;

/// A Ruby encoding, as Strings are read and converted by it.
pub mod encoding;

/// A Ruby String's bytes, read and written by copying.
pub mod string;

/// A private table from small integers to values ([`table::IntTable`]).
pub mod table;

/// Reading a method's arguments: a type check each, no conversion.
pub mod args;

/// Ruby values written into a message without a call.
pub mod describe;

/// The Ruby API version, built against and running.
pub mod abi;

/// The native-stack check, for a token.
pub mod stack;

/// `Msg`: an exception message written into a fixed buffer on the stack,
/// so an error path allocates nothing.
pub mod msg;

/// Catching a panic at a C boundary and re-raising it past one.
pub mod caught;

/// The layer's bodies that must not unwind, and the build that checks they
/// cannot (`rake no_unwind`).
pub mod no_unwind;
