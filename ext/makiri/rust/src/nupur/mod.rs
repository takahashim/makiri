//! `nupur`: the Ruby binding layer Makiri is built on - values, exceptions,
//! method trampolines, TypedData, the GVL, and every C call that can raise,
//! over rb-sys alone.
//!
//! It knows nothing of Makiri: it names no module of the crate outside
//! `crate::nupur` (`rake dylint`'s `nupur_names_its_host` holds it to that),
//! and what it needs of the extension - the exception a broken invariant
//! raises, the raise sweep's hook - is registered at `Init_`
//! ([`helpers::INTERNAL_ERROR`], `raw::set_raise_injector`). Stage 1 of
//! `notes/nupur_crate_plan.ja.md`: a module of the extension's crate, on its
//! way to a crate of its own. `bridge` re-exports it under its old names, so
//! the rest of the extension is unchanged.

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
