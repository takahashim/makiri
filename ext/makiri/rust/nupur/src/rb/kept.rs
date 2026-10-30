//! [`Kept`]: a Ruby value held where the GC's stack scan finds it until the
//! holder is dropped - what a `typed::Typed` handle borrows its object's
//! data through - and [`keep_alive`], the empty `asm!` block that puts it
//! there.

#![allow(unsafe_code)]

use rb_sys::VALUE;

use super::value::{AsRawValue, ReprValue, Value};

/// A Ruby value held where the GC's scan finds it until this is dropped -
/// what a `typed::Typed` handle holds, so the object stays while its data is
/// borrowed, whether Ruby keeps it as well or this frame alone does (an
/// object just made). The layer's own.
///
/// A borrow of an object's data ([`crate::typed::TypedType::get`],
/// [`crate::typed::Typed::data`]) keeps its `Value` alive to the type checker
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

/// Hand `v` to an empty inline assembly block as an input operand, so it is
/// in a register at this point.
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
fn keep_alive(v: Value) {
    let raw: VALUE = v.as_raw();
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
fn keep_alive(v: Value) {
    let raw: VALUE = v.as_raw();
    // SAFETY: `raw` is a local of this frame, aligned and initialised, and
    // read once; reading a `VALUE` (an integer) has no other effect.
    unsafe {
        core::ptr::read_volatile(&raw);
    }
}
