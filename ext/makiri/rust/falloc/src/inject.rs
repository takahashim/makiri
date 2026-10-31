//! The injection counters the failure sweeps arm: one for the `rake oom`
//! sweep's allocation failures (`script/check_alloc_failures.rb`), one for
//! its Ruby-raise sweep (`script/check_raise_injection.rb`), which makes a
//! raising C call of `nupur::raw` raise `NoMemoryError` in place of its
//! allocation.
//!
//! The counters are atomic: a parse consults the allocation one under
//! `rb_thread_call_without_gvl`, so two threads can reach it at once. None of
//! these calls is memory-unsafe - misusing the hooks mis-sizes or confuses the
//! sweep, never triggers UB - so they are ordinary functions whose contract is
//! stated in prose rather than `unsafe`.

use core::sync::atomic::{AtomicU64, Ordering};

/// One armable "fail the nth consultation once" counter.
///
/// The countdown and the attempt counter in ONE word, so arming both is a
/// single store: two separate stores would let a consultation on another thread
/// land between them and be counted against the wrong run.
///
/// Bits 0..32 hold the remaining countdown (0 disarms); bits 32..64 hold the
/// number of consultations since the last arm. An attempt count past `u32::MAX`
/// wraps - a run that large is far outside what the sweep drives.
struct Injector(AtomicU64);

/// The low half: the countdown field.
const COUNTDOWN_MASK: u64 = u32::MAX as u64;

impl Injector {
    const fn new() -> Self {
        Injector(AtomicU64::new(0))
    }

    /// Arm "the `nth` consultation fails once" (`nth <= 0` disarms), and reset
    /// the attempt counter the sweep sizes itself from.
    fn arm(&self, nth: i64) {
        let countdown = nth.clamp(0, COUNTDOWN_MASK as i64) as u64;
        self.0.store(countdown, Ordering::Release);
    }

    /// The number of consultations since the last arm.
    fn calls(&self) -> u64 {
        self.0.load(Ordering::Acquire) >> 32
    }

    /// Count a consultation, and answer whether it is the armed one.
    fn consult(&self) -> bool {
        /* One read-modify-write: bump the attempt count and drop the countdown.
         * The consultation that takes the countdown from 1 to 0 is the one that
         * fails. Written as the compare-exchange loop rather than
         * `fetch_update`, which newer toolchains deprecate for `try_update` - a
         * name older ones lack. */
        let mut state = self.0.load(Ordering::Acquire);
        loop {
            let attempts = (state >> 32).wrapping_add(1);
            let countdown = state & COUNTDOWN_MASK;
            let next = (attempts << 32) | countdown.saturating_sub(1);
            match self
                .0
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return countdown == 1,
                Err(seen) => state = seen,
            }
        }
    }
}

static ALLOC: Injector = Injector::new();
static RAISE: Injector = Injector::new();

/// Arm "the `nth` hook consultation fails once" (`nth <= 0` disarms), and reset
/// the attempt counter the sweep sizes itself from.
///
/// Only the serialized OOM sweep should call this: arming it under a live
/// workload would fail an allocation no test asked for.
pub fn alloc_inject_arm(nth: i64) {
    ALLOC.arm(nth);
}

/// Return the number of hook consultations since the last arm.
pub fn alloc_inject_call_count() -> u64 {
    ALLOC.calls()
}

/// Consult the shared counter and fail the armed consultation, if any.
///
/// The caller's only contract is to consult it once per allocation attempt;
/// violating that mis-numbers the sweep, which is not a soundness problem.
pub fn alloc_inject_should_fail() -> bool {
    ALLOC.consult()
}

/// Arm "the `nth` raising C call raises `NoMemoryError` instead" (`nth <= 0`
/// disarms), and reset its attempt counter. The raise sweep's.
pub fn raise_inject_arm(nth: i64) {
    RAISE.arm(nth);
}

/// The number of raising C calls consulted since the last arm.
pub fn raise_inject_call_count() -> u64 {
    RAISE.calls()
}

/// Count a raising C call, and answer whether it is the armed one -
/// `nupur::raw`'s allocating wrappers ask, once per call.
pub fn raise_inject_should_raise() -> bool {
    RAISE.consult()
}
