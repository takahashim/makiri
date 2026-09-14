//! Shared OOM injection counter used by the `rake oom` sweep.

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

static COUNTDOWN: AtomicI64 = AtomicI64::new(0);
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);

/// Arm the next allocation attempt to fail once.
///
/// # Safety
/// This test hook must only be used by the serialized OOM sweep.
pub unsafe fn mkr_alloc_inject_arm(nth: i64) {
    COUNTDOWN.store(if nth > 0 { nth } else { 0 }, Ordering::Release);
    ATTEMPTS.store(0, Ordering::Release);
}

/// Return the number of allocation attempts since the last arm.
///
/// # Safety
/// This test hook has no memory-safety preconditions; callers must only use
/// the result for the matching OOM sweep.
pub unsafe fn mkr_alloc_inject_calls() -> u64 {
    ATTEMPTS.load(Ordering::Acquire)
}

/// Consult the shared counter and fail the armed attempt, if any.
///
/// # Safety
/// The hook must be called once per allocation attempt and must not be used to
/// replace the allocator's normal failure handling.
pub unsafe fn mkr_alloc_inject_should_fail() -> core::ffi::c_int {
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
