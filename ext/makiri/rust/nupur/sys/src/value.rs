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

use rb_sys::VALUE;

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

    /// The same object (or immediate) as `other`?
    #[inline(always)]
    pub fn is(self, other: Value) -> bool {
        self.0 == other.0
    }

    /// The value's word, as an identity: the same for every reference to one
    /// object, different for two live objects - a key to compare by, never a
    /// value to turn back into one.
    #[inline(always)]
    pub fn identity(self) -> usize {
        crate::int::word(self.0)
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

/// A Ruby value kept in a TypedData object's slot: on the heap, where the
/// stack scan does not see it and the object's mark callback marks it
/// ([`crate::read::gc_mark_in_dmark`]). The one form in which a value may
/// live outside a Rust local.
///
/// `repr(transparent)`, and `!Send`, as a value is. Made from a value, which
/// is safe - it is a value's bits, kept - and read back as one only under a
/// contract, since a slot read where its object is gone is a stale word.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Root(VALUE, PhantomData<*mut ()>);

impl Root {
    /// `v`, to be kept in a slot.
    #[inline(always)]
    pub fn of(v: Value) -> Root {
        Root(v.0, PhantomData)
    }

    /// The value this slot keeps.
    ///
    /// # Safety
    ///
    /// The slot is one of a live object's, which its mark callback marks, read
    /// on a Ruby thread holding the GVL.
    #[inline(always)]
    pub unsafe fn value(self) -> Value {
        Value(self.0, PhantomData)
    }

    /// The word, for the mark.
    #[inline(always)]
    pub(crate) fn as_raw(self) -> VALUE {
        self.0
    }
}

/// Keep `v` alive to this point (CRuby's `RB_GC_GUARD`): hand it to an empty
/// inline assembly block as an input operand, so it is in a register here.
///
/// The contract relied on is the Rust Reference's for `asm!`, and only this
/// much of it: an input operand is in its register when the block runs, and
/// a block without `pure` is taken to have side effects and is not removed.
/// Without `nomem` or `readonly` the block may read and write memory, as may
/// the libruby calls that can collect, so the two stay in program order.
/// (`core::hint::black_box`, which this replaced, is documented as
/// best-effort, not to be relied on.)
///
/// That `v` is ALSO where the scan finds it at each collection before this
/// point follows from premises, not from the block: a collection runs only
/// inside a call into libruby, with the GVL held; those calls are opaque to
/// the optimizer (libruby is linked dynamically, with no LTO across it), so
/// a value used after one is kept across it in a callee-saved register or a
/// stack slot; and CRuby's mark saves the registers with `setjmp` and scans
/// the machine stack, which finds either.
#[cfg(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "arm",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64",
    target_arch = "s390x"
))]
#[inline(always)]
pub fn keep_alive(v: Value) {
    let raw: VALUE = v.0;
    // SAFETY: the template is a comment: no instruction, no register or
    // memory written, the stack untouched (`nostack`) and the flags kept.
    unsafe {
        core::arch::asm!("/* keep {0} */", in(reg) raw, options(nostack, preserves_flags));
    }
}

/// [`keep_alive`] where `asm!` is not stable (PowerPC, MIPS, SPARC, ...):
/// `v` put in a local and read back with `read_volatile` - the means rb-sys's
/// `rb_gc_guard!` uses, written here so its `unsafe` is this file's, counted
/// by `rake unsafe:boundaries`.
///
/// What it relies on is weaker than the `asm!` contract: `core::ptr`'s
/// documentation says a volatile access is not elided or reordered with
/// other volatile accesses, and also that Rust's volatile has no rigorously
/// defined semantics yet ("pretty similar to C11's"). The read needs the
/// value in the local at that point, which is what the `asm!` block gives,
/// and the same premises carry it back to each collection before. Unlike the
/// `asm!` targets, the generated code has not been checked on any of these.
/// (`rb_gc_guarded_ptr_val`, CRuby's out-of-line `RB_GC_GUARD`, is no
/// fallback: libruby has it only when built by neither GCC, clang nor MSVC.)
#[cfg(not(any(
    target_arch = "x86",
    target_arch = "x86_64",
    target_arch = "arm",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64",
    target_arch = "s390x"
)))]
#[inline(always)]
pub fn keep_alive(v: Value) {
    let raw: VALUE = v.0;
    // SAFETY: `raw` is a local of this frame, aligned and initialised, and
    // read once; reading a `VALUE` (an integer) has no other effect.
    unsafe {
        core::ptr::read_volatile(&raw);
    }
}
