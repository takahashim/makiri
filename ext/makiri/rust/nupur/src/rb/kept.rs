//! [`Kept`]: a Ruby value held where the GC's stack scan finds it until the
//! holder is dropped - what a `typed::Typed` handle borrows its object's
//! data through - and [`keep_alive`], the empty `asm!` block that puts it
//! there.

#![forbid(unsafe_code)]

use super::value::{AsSys, ReprValue, Value};

/// A Ruby value held where the GC's scan finds it until this is dropped -
/// what a `typed::Typed` handle holds, so the object stays while its data is
/// borrowed, whether Ruby keeps it as well or this frame alone does (an
/// object just made). The layer's own.
///
/// A borrow of an object's data ([`crate::typed::Typed::data`]) would keep
/// a bare `Value` alive to the type checker
/// only: once the `Value` is no longer used, the optimizer may drop it from
/// every register and stack slot, and the data pointer left - a non-embedded
/// type's data is its own malloc'd block - is not one the scan takes for the
/// object. A receiver or an argument is kept by Ruby's own stack, and a
/// slot's value by its owner; an object nothing else refers to yet is not,
/// and a handle holds either the same way. Its `Drop` hands the value to
/// [`keep_alive`], so do not `mem::forget` one.
pub(crate) struct Kept(Value);

impl Kept {
    /// Hold `v`.
    #[inline]
    pub(crate) fn new(v: impl ReprValue) -> Kept {
        Kept(v.as_value())
    }

    /// The value, borrowed from the holder: a borrow of the object's data
    /// taken through it cannot outlive the `Kept`.
    #[inline]
    pub(crate) fn value(&self) -> &Value {
        &self.0
    }
}

impl Drop for Kept {
    #[inline]
    fn drop(&mut self) {
        let v = self.0;
        crate::no_unwind::must_not_unwind::<crate::no_unwind::DropKept, _>(|| keep_alive(v))
    }
}

/// Keep `v` alive to this point (CRuby's `RB_GC_GUARD`): `nupur_sys`'s
/// `keep_alive`, an empty `asm!` that takes the value, or a volatile read
/// where `asm!` is not stable.
#[inline(always)]
fn keep_alive(v: Value) {
    nupur_sys::value::keep_alive(v.as_sys());
}
