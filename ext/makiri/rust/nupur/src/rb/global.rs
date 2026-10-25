//! Values kept for the rest of the process, rooted with the GC: [`Opaque`],
//! and [`RbConst`], the class handles an extension defines at `Init_` and
//! reads in its methods.

#![allow(unsafe_code)]

use std::sync::OnceLock;

use super::error::{Builtin, Error};
use super::ruby::Ruby;
use super::value::{ExceptionClass, ReprValue, Value};

/// A value kept for the rest of the process: registered with the GC as a
/// mark root, so it is never collected or moved, and readable from any
/// thread that holds the GVL ([`Opaque::get`]). What the class table in
/// an extension's `Init_` registers are made of.
#[derive(Clone, Copy)]
pub struct Opaque<T>(T);

// SAFETY: an `Opaque` is read only through `Opaque::get`, which takes the
// GVL's proof; the value itself is pinned and rooted by `register`.
unsafe impl<T> Send for Opaque<T> {}
// SAFETY: as `Send`.
unsafe impl<T> Sync for Opaque<T> {}

impl<T: ReprValue> Opaque<T> {
    /// `v`, rooted with the GC for the rest of the process.
    pub fn register(v: T) -> Result<Opaque<T>, Error> {
        crate::raw::gc_register_mark_object(v.as_value())?;
        Ok(Opaque(v))
    }
}

impl<T: Copy> Opaque<T> {
    /// The value it keeps, read with the GVL held.
    #[inline]
    pub fn get(self, _: &Ruby) -> T {
        self.0
    }
}

/// A class, module or exception class an extension's `Init_` defines, for its methods to read.
///
/// Typed: an `RbConst<RClass>` can only ever hold an `RClass`, so a read hands
/// the handle back with no type check to fail. It is written once, at `Init_`,
/// which also registers the object with the GC - so it stays alive and unmoved
/// even if Ruby code removes the constant that named it.
///
/// Unset is the state before the extension's `Init_`, when none of the methods that read
/// one exist yet. A read in that state answers `None` (or the documented
/// fallback of the reader below) rather than panicking.
pub struct RbConst<T>(OnceLock<Opaque<T>>);

impl<T: ReprValue> Default for RbConst<T> {
    fn default() -> RbConst<T> {
        RbConst::new()
    }
}

impl<T: ReprValue> RbConst<T> {
    pub const fn new() -> RbConst<T> {
        RbConst(OnceLock::new())
    }

    /// Record `v`, rooted for the rest of the process. `Init_` is the only
    /// writer, and a second write is refused rather than replacing the first.
    pub fn set(&self, v: T) -> Result<(), Error> {
        self.0
            .set(Opaque::register(v)?)
            .map_err(|_| Error::new(Builtin::RuntimeError, "a constant handle was set twice"))
    }

    /// The object, or `None` before the extension's `Init_`.
    #[inline]
    pub fn get(&self, ruby: &Ruby) -> Option<T> {
        self.0.get().map(|o| o.get(ruby))
    }

    /// The object, for the registration code `Init_` runs once it is set.
    pub fn defined(&self, ruby: &Ruby) -> Result<T, Error> {
        self.get(ruby).ok_or_else(|| {
            Error::new(
                Builtin::RuntimeError,
                "a constant handle was read before the extension initialised",
            )
        })
    }
}

impl RbConst<ExceptionClass> {
    /// The class to raise. Before the extension's `Init_` - when nothing that raises one
    /// of these can run - it is `RuntimeError`, so a raise still raises.
    #[inline]
    pub fn exception(&self, ruby: &Ruby) -> ExceptionClass {
        self.get(ruby)
            .unwrap_or_else(|| ruby.exception_runtime_error())
    }
}

/// Is `v` an instance of the class in `klass`? `false` before the extension's `Init_`,
/// when no class of ours exists for it to be an instance of.
#[inline]
pub fn is_kind_of(ruby: &Ruby, v: Value, klass: &RbConst<crate::rb::RClass>) -> bool {
    klass.get(ruby).is_some_and(|c| v.is_kind_of(c))
}
