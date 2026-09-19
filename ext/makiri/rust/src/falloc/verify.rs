//! Kani proofs for the allocation sizing.
//!
//! The live remnant of `verify/harness_alloc.c`. Most of that harness is gone
//! rather than moved: `mkr_size_add` / `mkr_size_mul` were hand-written
//! overflow guards, and Rust spells them `checked_add` / `checked_mul`, so
//! there is no guard left to be wrong. That is a strengthening, not a
//! substitution - the C proof of the multiply had a documented gap (it closed
//! only for a dozen concrete element sizes, because relating the guard's 64-bit
//! division to the multiplication at full width is intractable for bit-level
//! solvers), and `checked_mul` has no such gap because it is not a guard at
//! all.
//!
//! What did NOT come free is the growth policy, which is ours either way.

#![forbid(unsafe_code)]
#![cfg(kani)]

use super::grow_capacity;

/// The growth contract, over arbitrary `cap` and `need` at a realistic element
/// size.
///
/// The properties are the ones a caller relies on:
///   - the answer covers `need` (otherwise the caller overflows its own array);
///   - the answer times the element size does not overflow (otherwise the
///     allocation that follows computes a wrong size);
///   - growth never shrinks an existing allocation below what it had.
///
/// `elem` is a fixed 8 here, not nondeterministic. The one caller passes
/// `size_of::<*mut c_void>()`, and a nondet `elem` puts the proof back into the
/// same multiply-versus-divide space that defeated the C version - the
/// restriction is faithful to the code rather than a concession to the solver.
#[kani::proof]
#[kani::unwind(80)]
fn grow_capacity_covers_need_without_overflow() {
    const ELEM: usize = 8;
    let cap: usize = kani::any();
    let need: usize = kani::any();

    match grow_capacity(cap, need, ELEM) {
        Some(nc) => {
            assert!(nc >= need, "the new capacity must cover what was needed");
            assert!(
                nc.checked_mul(ELEM).is_some(),
                "the byte size of the new capacity must not overflow"
            );
            // Only for a `cap` that could describe a live allocation AND a
            // non-empty request. An empty request deliberately returns 0
            // ("no allocation is required for an empty request"), which is
            // smaller than any live `cap`; that is a contract exception, not a
            // shrink.
            if cap != 0 && cap >= need && need > 0 && cap.checked_mul(ELEM).is_some() {
                assert!(nc >= cap, "growth must never shrink a live allocation");
            }
        }
        None => {
            // The only stated failure: `need` itself does not fit.
            assert!(
                need.checked_mul(ELEM).is_none(),
                "the only failure is need not fitting"
            );
        }
    }
}
