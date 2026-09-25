//! Shared OOM injection counter used by the `rake oom` sweep.
//!
//! The counter is atomic: a parse consults it under
//! `rb_thread_call_without_gvl`, so two threads can reach it at once. None of
//! these calls is memory-unsafe - misusing the hooks mis-sizes or confuses the
//! sweep, never triggers UB - so they are ordinary functions whose contract is
//! stated in prose rather than `unsafe`.

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

static COUNTDOWN: AtomicI64 = AtomicI64::new(0);
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);

/// Arm "the `nth` hook consultation fails once" (`nth <= 0` disarms), and reset
/// the attempt counter the sweep sizes itself from.
///
/// Only the serialized OOM sweep should call this: arming it under a live
/// workload would fail an allocation no test asked for.
pub fn alloc_inject_arm(nth: i64) {
    COUNTDOWN.store(nth.max(0), Ordering::Release);
    ATTEMPTS.store(0, Ordering::Release);
}

/// Return the number of hook consultations since the last arm.
pub fn alloc_inject_call_count() -> u64 {
    ATTEMPTS.load(Ordering::Acquire)
}

/// Consult the shared counter and fail the armed consultation, if any.
///
/// The caller's only contract is to consult it once per allocation attempt;
/// violating that mis-numbers the sweep, which is not a soundness problem.
pub fn alloc_inject_should_fail() -> bool {
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    /* Count down while armed; the consultation that takes it from 1 to 0 is
     * the one that fails. */
    COUNTDOWN.fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
        (left > 0).then(|| left - 1)
    }) == Ok(1)
}
