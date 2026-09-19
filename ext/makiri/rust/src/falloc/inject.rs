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
    COUNTDOWN.store(if nth > 0 { nth } else { 0 }, Ordering::Release);
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
pub fn alloc_inject_should_fail() -> core::ffi::c_int {
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    let mut left = COUNTDOWN.load(Ordering::Acquire);
    while left > 0 {
        match COUNTDOWN.compare_exchange_weak(left, left - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) if left == 1 => return 1,
            Ok(_) => return 0,
            Err(actual) => left = actual,
        }
    }
    0
}
