//! The two proofs that code runs on a Ruby thread holding the GVL, which
//! every read of Ruby's C here takes ([`crate::read`]): a Ruby value
//! ([`Value`]), for what reads one, and the GVL's token ([`Ruby`]), for what
//! reads none.
//!
//! A value is the proof because one exists only where Ruby runs: it is made
//! by an `unsafe fn` whose contract says so, and it is `!Send`, so a body
//! run with the GVL released cannot carry one. The token is made the same
//! way, where Ruby calls in (a method's trampoline, `Init_`, a GC callback),
//! and a value or the protection lends one ([`Value::ruby`],
//! [`crate::protect::Protected::ruby`]) for as long as it is borrowed.

#![allow(unsafe_code)]

use core::marker::PhantomData;

use crate::VALUE;

/// A live Ruby value, of any kind: on a Ruby thread holding the GVL, and
/// kept where the GC finds it (a Rust local, which the stack scan sees) for
/// as long as it is used.
///
/// `repr(transparent)`, so it is a `VALUE` to C. Made only by
/// [`Value::from_raw`] outside this crate:
///
/// ```compile_fail,E0423
/// let made = nupur_sys::value::Value(0, core::marker::PhantomData);
/// ```
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Value(VALUE, PhantomData<*mut ()>);

impl Value {
    /// The value a C call returned, or one Ruby passed in.
    ///
    /// # Safety
    ///
    /// `v` is a live Ruby value, on a Ruby thread holding the GVL, kept where
    /// the GC finds it for as long as the `Value` is used.
    #[inline(always)]
    pub const unsafe fn from_raw(v: VALUE) -> Value {
        Value(v, PhantomData)
    }

    /// The raw `VALUE`, for a C call.
    #[inline(always)]
    pub const fn as_raw(self) -> VALUE {
        self.0
    }

    /// `Qundef`?
    #[inline(always)]
    pub fn is_undef(self) -> bool {
        self.0 == rb_sys::Qundef as VALUE
    }

    /// `true`?
    #[inline(always)]
    pub fn is_true(self) -> bool {
        self.0 == rb_sys::Qtrue as VALUE
    }

    /// The GVL's token this value proves, for a read that takes no value
    /// (a C global's class, say), lent for the borrow of the value.
    #[inline(always)]
    pub fn ruby(&self) -> &Ruby {
        &TOKEN
    }
}

/// The GVL, held: a Ruby thread, between Ruby's call in and its return.
///
/// A zero-sized token, neither `Send` nor `Sync`, made only by
/// [`Ruby::get_unchecked`] outside this crate:
///
/// ```compile_fail,E0423
/// let made = nupur_sys::value::Ruby(core::marker::PhantomData);
/// ```
pub struct Ruby(PhantomData<*mut ()>);

/// The one token a value or the protection lends.
const TOKEN: Ruby = Ruby(PhantomData);

impl Ruby {
    /// The token, unchecked.
    ///
    /// # Safety
    ///
    /// On a Ruby thread holding the GVL, for as long as the token is used.
    #[inline(always)]
    pub unsafe fn get_unchecked() -> Ruby {
        Ruby(PhantomData)
    }

    /// `nil`.
    #[inline(always)]
    pub fn qnil(&self) -> Value {
        // SAFETY: an immediate, and the token is the GVL.
        unsafe { Value::from_raw(rb_sys::Qnil as VALUE) }
    }

    /// `true`.
    #[inline(always)]
    pub fn qtrue(&self) -> Value {
        // SAFETY: as `qnil`.
        unsafe { Value::from_raw(rb_sys::Qtrue as VALUE) }
    }

    /// `false`.
    #[inline(always)]
    pub fn qfalse(&self) -> Value {
        // SAFETY: as `qnil`.
        unsafe { Value::from_raw(rb_sys::Qfalse as VALUE) }
    }

    /// `Qundef`, the C API's "no value" - never a Ruby object, and never
    /// handed to Ruby code; a lookup's default, compared with
    /// [`Value::is_undef`].
    #[inline(always)]
    pub fn qundef(&self) -> Value {
        // SAFETY: as `qnil`.
        unsafe { Value::from_raw(rb_sys::Qundef as VALUE) }
    }

    /// The Fixnum `n` is (`LONG2FIX`), or `None` past a Fixnum's range
    /// ([`crate::int::fixnum`]).
    #[inline(always)]
    pub fn fixnum(&self, n: i64) -> Option<Value> {
        // SAFETY: an immediate, built from its bits.
        crate::int::fixnum(n).map(|v| unsafe { Value::from_raw(v) })
    }

    /// [`Ruby::fixnum`] for an unsigned value.
    #[inline(always)]
    pub fn posfixnum(&self, n: u64) -> Option<Value> {
        // SAFETY: as `fixnum`.
        crate::int::posfixnum(n).map(|v| unsafe { Value::from_raw(v) })
    }

    /// The token, lent for the protection's borrow (`Protected::ruby`).
    #[inline(always)]
    pub(crate) fn lent() -> &'static Ruby {
        &TOKEN
    }
}
