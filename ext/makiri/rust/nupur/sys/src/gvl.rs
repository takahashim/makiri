//! The proof that code runs on a Ruby thread holding the GVL: [`Gvl`], which
//! every read of Ruby's C here takes ([`crate::read`]).
//!
//! The proof is made in three places only. [`Gvl::assume`] is the `unsafe fn`
//! whose contract is that very fact: Nupur calls it where Ruby has called in
//! with the GVL held (a method's trampoline, `Init_`, a GC callback) and where
//! a Ruby value shows it (a value exists only where Ruby runs). Inside the
//! protection, [`crate::protect::Protected::gvl`] lends it, since `rb_protect`
//! runs only under the GVL.

#![allow(unsafe_code)]

use core::marker::PhantomData;

/// The proof that the thread is a Ruby thread holding the GVL.
///
/// Neither `Send` nor `Sync`, so a body run with the GVL released cannot
/// carry it, and made only by an `unsafe fn` outside this crate:
///
/// ```compile_fail,E0451
/// let made = nupur_sys::gvl::Gvl { _not_send: core::marker::PhantomData };
/// ```
pub struct Gvl {
    _not_send: PhantomData<*mut ()>,
}

/// The one proof, lent for as long as the caller says it holds.
const PROOF: Gvl = Gvl {
    _not_send: PhantomData,
};

impl Gvl {
    /// The proof, taken on the caller's word.
    ///
    /// # Safety
    ///
    /// The thread is a Ruby thread holding the GVL, for as long as the proof
    /// is used: the caller keeps it on its own frame, which a GVL-released
    /// body cannot reach (the proof is `!Send`).
    #[inline(always)]
    pub unsafe fn assume() -> &'static Gvl {
        &PROOF
    }
}
