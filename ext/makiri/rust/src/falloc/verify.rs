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

/// The growth contract, checked for one fixed element size.
///
/// The properties are the ones a caller relies on:
///   - the answer covers `need` (otherwise the caller overflows its own array);
///   - the answer times the element size does not overflow (otherwise the
///     allocation that follows computes a wrong size);
///   - growth never shrinks an existing allocation below what it had.
///
/// `ELEM` is a const, not nondeterministic. `grow_capacity`'s size arithmetic
/// is `checked_mul(elem)` several times, once per iteration of the doubling
/// loop; with a constant `elem` each is a multiply by a constant (a shift when
/// it is a power of two, which every real element size here is), but a
/// nondeterministic `elem` makes them symbolic-by-symbolic multiplies, and
/// bit-blasting a 64-bit multiplier inside an unwound loop does not finish. So
/// the proof runs once per element size a caller actually passes rather than
/// quantifying over all of them - the live callers are 1 (the byte buffer) and
/// `size_of::<usize>()` (the node set's word).
fn holds_for<const ELEM: usize>() {
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

/// `elem == 1`: `Buf` (the byte buffer, the most-used path).
#[kani::proof]
#[kani::unwind(80)]
fn grow_capacity_covers_need_without_overflow_bytes() {
    holds_for::<1>();
}

/// `elem == size_of::<usize>()`: `NodeSet` (`NodeWord`, a `usize`).
#[kani::proof]
#[kani::unwind(80)]
fn grow_capacity_covers_need_without_overflow_words() {
    holds_for::<{ core::mem::size_of::<usize>() }>();
}
