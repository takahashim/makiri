//! Shared OOM injection counter used by the `rake oom` sweep.
//!
//! The counter is atomic: a parse consults it under
//! `rb_thread_call_without_gvl`, so two threads can reach it at once. None of
//! these calls is memory-unsafe - misusing the hooks mis-sizes or confuses the
//! sweep, never triggers UB - so they are ordinary functions whose contract is
//! stated in prose rather than `unsafe`.

use core::sync::atomic::{AtomicU64, Ordering};

/// The countdown and the attempt counter in ONE word, so arming both is a
/// single store: two separate stores would let a consultation on another thread
/// land between them and be counted against the wrong run.
///
/// Bits 0..32 hold the remaining countdown (0 disarms); bits 32..64 hold the
/// number of consultations since the last arm. An attempt count past `u32::MAX`
/// wraps - a run that large is far outside what the sweep drives.
static STATE: AtomicU64 = AtomicU64::new(0);

/// The low half: the countdown field.
const COUNTDOWN_MASK: u64 = u32::MAX as u64;

/// Arm "the `nth` hook consultation fails once" (`nth <= 0` disarms), and reset
/// the attempt counter the sweep sizes itself from.
///
/// Only the serialized OOM sweep should call this: arming it under a live
/// workload would fail an allocation no test asked for.
pub fn alloc_inject_arm(nth: i64) {
    let countdown = nth.clamp(0, COUNTDOWN_MASK as i64) as u64;
    STATE.store(countdown, Ordering::Release);
}

/// Return the number of hook consultations since the last arm.
pub fn alloc_inject_call_count() -> u64 {
    STATE.load(Ordering::Acquire) >> 32
}

/// Consult the shared counter and fail the armed consultation, if any.
///
/// The caller's only contract is to consult it once per allocation attempt;
/// violating that mis-numbers the sweep, which is not a soundness problem.
pub fn alloc_inject_should_fail() -> bool {
    /* One read-modify-write: bump the attempt count and drop the countdown. The
     * consultation that takes the countdown from 1 to 0 is the one that fails. */
    let previous = STATE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
        let attempts = (state >> 32).wrapping_add(1);
        let countdown = state & COUNTDOWN_MASK;
        Some((attempts << 32) | countdown.saturating_sub(1))
    });
    match previous {
        Ok(state) => (state & COUNTDOWN_MASK) == 1,
        // Unreachable: the closure always returns `Some`.
        Err(_) => false,
    }
}
