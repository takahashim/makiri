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

pub use rb_sys::*;

pub mod no_unwind;
pub mod protect;

/* `rake dylint`'s self-test: what the lints must report here. */
#[cfg(nupur_sys_lint_probe)]
mod lint_probe;
