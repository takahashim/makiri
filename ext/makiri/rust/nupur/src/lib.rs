//! `nupur`: the Ruby binding layer Makiri is built on - values, exceptions,
//! method trampolines, TypedData, the GVL, and every C call that can raise,
//! over rb-sys alone.
//!
//! It knows nothing of Makiri - it is a crate of its own, which does not
//! depend on the extension - and what it needs of the extension (the
//! exception a broken invariant raises, the raise sweep's hook) is
//! registered at `Init_` ([`helpers::INTERNAL_ERROR`],
//! `raw::set_raise_injector`). `notes/nupur_crate_plan.ja.md` is its plan.
//!
//! Every `unsafe` the extension needs to talk to Ruby is here, each with its
//! contract stated; what it hands out is safe. The crate denies `unsafe_code`
//! and the files that need it carry an `allow` each, which `rake
//! unsafe:boundaries` counts.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
/* `rake dylint` sets `dylint_lib` for the whole build, the extension's run
 * compiling this crate as a plain dependency: there the extension's lints,
 * which the `allow`s here name, are unknown. */
#![cfg_attr(dylint_lib = "makiri_lints", allow(unknown_lints))]

/// The lints' self-test for this crate (`rake dylint`).
#[cfg(makiri_lint_probe)]
mod lint_probe;

/// The value and method layer: `Value` and the class-checked handles,
/// `Error`, the `method!` / `function!` trampolines, class definitions.
pub mod rb;

/// Every Ruby C call that can raise, each under `rb_protect`.
pub mod raw;

/// TypedData: wrapping a Rust value as a Ruby object, and its GC hooks.
pub mod typed;

/// Running a body with the GVL released.
pub mod gvl;

/// Building Ruby values: Arrays, Hashes, numbers, Symbols, UTF-8 Strings.
pub mod build;

/// A Ruby encoding, as Strings are read and converted by it.
pub mod encoding;

/// The shared helpers: argument checks, the common exceptions, `RbConst`.
pub mod helpers;

/// `Msg`: an exception message written into a fixed buffer on the stack,
/// so an error path allocates nothing.
pub mod msg;

/// Catching a panic at a C boundary and re-raising it past one.
pub mod caught;

/// The layer's bodies that must not unwind, and the build that checks they
/// cannot (`rake no_unwind`).
pub mod no_unwind;
