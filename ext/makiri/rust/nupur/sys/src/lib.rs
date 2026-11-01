//! `nupur_sys`: Ruby's C API as Nupur sees it - the one crate of the layer
//! that depends on rb-sys, so Nupur itself cannot name rb-sys.
//!
//! It holds the protection every C call that can raise runs under
//! ([`protect`]: `rb_protect`, `$!`, a pending non-local exit, and the
//! [`protect::Protected`] token the protected closure is lent), and the
//! mechanism that proves what must not unwind cannot ([`no_unwind`]), whose
//! guard the protect thunk runs under.
//!
//! Beside them it re-exports rb-sys whole, for now, and Nupur calls the rest
//! of Ruby's C through it as it called rb-sys. The re-export changes no path
//! the lints resolve (an item is still `rb_sys::...` where it is defined).
//! The later steps move Nupur's wrappers here, each taking a proof of
//! `rb_protect` or of the GVL, so those rules become types rather than lints.
//!
//! The crate denies `unsafe_code`, and the files that need it carry an
//! `allow` each, as Nupur does.
#![deny(unsafe_code)]
/* The casts that lose a value, as in Nupur: where the code is built a C
 * `long` is `i64` itself, so these see nothing there; on Windows, whose
 * `long` is 32 bits, clippy sees the real types - CI runs it on that cell
 * too. `int` is the one module that casts. */
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
/* `i64::from(a_c_long)` is a no-op here and a widening on Windows: the
 * conversions are written for every target, which this lint cannot see. */
#![allow(
    clippy::useless_conversion,
    reason = "C's integer types are aliases whose width is the target's"
)]
/* Built as a dependency under `rake dylint`, the crate sees `dylint_lib` but
 * not the lints, so their `allow`s name lints it does not know. */
#![cfg_attr(dylint_lib = "makiri_lints", allow(unknown_lints))]

pub use rb_sys::*;

pub mod gvl;
pub mod int;
pub mod no_unwind;
pub mod protect;
pub mod raising;
pub mod read;

/* `rake dylint`'s self-test: what the lints must report here. */
#[cfg(nupur_sys_lint_probe)]
mod lint_probe;
