//! Fallible allocation: the one place Rust code is allowed to ask for memory.
//!
//! # Why this exists
//!
//! `rake oom` is the gate behind CLAUDE.md's fail-closed rule. It arms "the nth
//! core allocation fails", runs a workload, and asserts the result is a clean
//! exception or byte-identical to the baseline - never truncated, never a
//! crash. The C half funnels every allocation through `core/mkr_alloc.c`, which
//! consults the injection hook, so the sweep reaches all of it.
//!
//! Rust was not in that sweep at all. Two things were wrong, and they are
//! different problems:
//!
//!  1. **Not injectable.** Allocations made with `std` containers never consult
//!     the hook, so no sweep could reach them. The XPath engine and the bridge
//!     were fine - they call `mkr_callocarray` / `mkr_reallocarray` /
//!     `mkr_grow_reserve` and inherit the hook - but everything built on `Vec`,
//!     `HashMap` and `Box` was invisible.
//!  2. **Not fallible.** `Box::new`, `Vec::push` and `HashMap::insert` abort the
//!     process on allocation failure (`handle_alloc_error`). For a library
//!     loaded into someone's application server, aborting is not "failing
//!     closed" - it takes the host down instead of raising. The result is
//!     safe (never a wrong answer) but the blast radius is the whole process.
//!
//! So this module does both: it consults the same counter the C half does, and
//! it returns failure instead of aborting. Every heap allocation in Rust code
//! that is not already going through the C allocator goes through here.
//!
//! # The counter is shared, deliberately
//!
//! `mkr_alloc_inject_should_fail` is the C hook itself, not a copy of it. One
//! counter means one sweep with one numbering: `rake oom` does not need to know
//! which language owns allocation number 4,271, and a sweep sized from a
//! disarmed baseline run stays correct as sites move from C to Rust. When
//! `core/` is ported (step 8 of notes/rust_port_remaining.ja.md) the counter
//! moves here and the direction of the call reverses; nothing else changes.
//!
//! # Two shapes, and why
//!
//! Growth is on traits (`Reserve`, `VecPush`, `MapInsert`) because it has a
//! receiver: `v.mkr_push(x)` reads like the `v.push(x)` it replaces, which is
//! what kept the conversion of nineteen call sites reviewable. Construction is
//! free functions (`try_box`, `try_vec_with_capacity`, `try_to_vec`, ...)
//! because there is nothing to hang a method on. Each names one shape and each
//! has callers; the split is by whether a receiver exists, not by accident.
//!
//! # Cost when not sweeping
//!
//! None. Without the `alloc-inject` feature `should_fail` is a `const false`
//! that the optimiser deletes along with the branch, exactly as the C macro
//! `MKR_ALLOC_INJECT_FAIL()` compiles to `0` outside `-DMKR_ALLOC_INJECT`.
//! extconf turns the feature on for the same `MAKIRI_ALLOC_INJECT=1` that
//! defines the C macro, so the two halves can never disagree about whether a
//! build is a sweep build.

// `Result<(), ()>` throughout: these report "the allocation failed", which
// carries no information beyond itself, and the crate already uses that shape
// for the same reason (see xml/chars.rs).
#![allow(clippy::result_unit_err)]

use std::collections::{HashMap, HashSet};

/// The C allocator surface (core/mkr_alloc.c), when this build provides it.
pub mod calloc;
/// Its Kani proofs - the ownership contract at the boundary, which is what is
/// left after the size arithmetic went to `checked_*` and the OOM branches to
/// `rake oom`.
pub mod calloc_verify;

/* The injection counter has ONE home, and which side that is depends on who
 * provides core/mkr_alloc.c. With `core-alloc` it is `calloc::inject`, and the
 * C reaches it through MKR_ALLOC_INJECT_FAIL(); without, it is still the C's
 * and this declaration reaches it. Either way `should_fail` below is the only
 * Rust entry, so there is no configuration in which two counters exist. */
#[cfg(all(feature = "alloc-inject", not(feature = "core-alloc")))]
extern "C" {
    /// `core/mkr_alloc.c`. Counts every attempt (armed or not) so the harness
    /// can size its sweep from a disarmed run, and fails exactly one.
    fn mkr_alloc_inject_should_fail() -> core::ffi::c_int;
}

#[cfg(all(feature = "alloc-inject", feature = "core-alloc"))]
use calloc::mkr_alloc_inject_should_fail;

/// Should this allocation be failed? Always false outside a sweep build.
#[cfg(feature = "alloc-inject")]
#[inline(always)]
pub fn should_fail() -> bool {
    // SAFETY: a plain counter read in C, no arguments, no pointers. The hook is
    // single-threaded by design (the sweep is), which holds here because every
    // caller is under the GVL.
    unsafe { mkr_alloc_inject_should_fail() != 0 }
}

/// Always false: a release build carries no counter and no branch.
#[cfg(not(feature = "alloc-inject"))]
#[inline(always)]
pub fn should_fail() -> bool {
    false
}

/// `Box::new` that reports failure instead of aborting.
///
/// Written against the raw allocator rather than `Box::try_new` (unstable): ask
/// for the layout, write the value into it, and only then claim ownership, so a
/// null response leaves `value` untouched and returns it to the caller. A
/// zero-sized `T` never allocates, so it cannot fail and is not counted - the
/// same convention as `mkr_callocarray(0, _)`.
#[inline]
pub fn try_box<T>(value: T) -> Result<Box<T>, T> {
    let layout = std::alloc::Layout::new::<T>();
    if layout.size() == 0 {
        #[allow(clippy::disallowed_methods)]
        return Ok(Box::new(value));
    }
    if should_fail() {
        return Err(value);
    }
    // SAFETY: the layout is non-zero-sized (checked above), so `alloc` is being
    // used within its contract. On success the pointer is fresh, uniquely
    // owned, aligned for T and uninitialised, which is exactly what `write`
    // needs and what `from_raw` then takes ownership of.
    unsafe {
        let p = std::alloc::alloc(layout) as *mut T;
        if p.is_null() {
            return Err(value);
        }
        p.write(value);
        Ok(Box::from_raw(p))
    }
}

/// `Box::into_raw(Box::new(v))` for the handles the C ABI hands out. Null is the
/// failure the C callers already expect from `mkr_callocarray`.
#[inline]
pub fn try_box_raw<T>(value: T) -> *mut T {
    match try_box(value) {
        Ok(b) => Box::into_raw(b),
        Err(_) => core::ptr::null_mut(),
    }
}

/// The fallible container operations, as one extension trait.
///
/// A trait rather than free functions because the call sites already read
/// `x.try_reserve(n).is_err()`; `x.mkr_reserve(n).is_err()` keeps that shape.
/// The `mkr_` prefix is deliberate: a method named `try_reserve` would be
/// shadowed by the inherent one silently, which is exactly the bug this module
/// exists to prevent.
///
/// Everything that grows a container is here, in one place. There was briefly a
/// second API - free `try_push` / `try_extend_from_slice` beside these methods -
/// which left a new call site with two equally-correct ways to spell the same
/// thing and no reason to pick either. The free functions that remain
/// (`try_box`, `try_vec_with_capacity`, `try_to_vec`) are constructors, which
/// have no receiver to hang a method on.
///
/// `clippy.toml` disallows the std methods these wrap, so a new site cannot
/// quietly go back to allocating outside the sweep.
pub trait Reserve {
    /// Room for `additional` more elements. `Err(())` leaves the receiver
    /// untouched.
    fn mkr_reserve(&mut self, additional: usize) -> Result<(), ()>;
    /// As `mkr_reserve`, without the growth slack.
    fn mkr_reserve_exact(&mut self, additional: usize) -> Result<(), ()>;
}

/// Growing a `Vec`, beyond the reserve itself.
pub trait VecPush<T> {
    /// Push one element. `Err(())` leaves the vector unchanged.
    fn mkr_push(&mut self, item: T) -> Result<(), ()>;
    /// Append a slice. `Err(())` leaves the vector unchanged.
    fn mkr_extend(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Clone;
}

impl<T> VecPush<T> for Vec<T> {
    #[inline]
    fn mkr_push(&mut self, item: T) -> Result<(), ()> {
        self.mkr_reserve(1)?;
        self.push(item);
        Ok(())
    }
    #[inline]
    fn mkr_extend(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Clone,
    {
        self.mkr_reserve(s.len())?;
        self.extend_from_slice(s);
        Ok(())
    }
}

impl<T> Reserve for Vec<T> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn mkr_reserve(&mut self, additional: usize) -> Result<(), ()> {
        if should_fail() {
            return Err(());
        }
        self.try_reserve(additional).map_err(|_| ())
    }
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn mkr_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        if should_fail() {
            return Err(());
        }
        self.try_reserve_exact(additional).map_err(|_| ())
    }
}

impl<K: core::hash::Hash + Eq, V, S: core::hash::BuildHasher> Reserve for HashMap<K, V, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn mkr_reserve(&mut self, additional: usize) -> Result<(), ()> {
        if should_fail() {
            return Err(());
        }
        self.try_reserve(additional).map_err(|_| ())
    }
    #[inline]
    fn mkr_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        self.mkr_reserve(additional)
    }
}

impl<T: core::hash::Hash + Eq, S: core::hash::BuildHasher> Reserve for HashSet<T, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn mkr_reserve(&mut self, additional: usize) -> Result<(), ()> {
        if should_fail() {
            return Err(());
        }
        self.try_reserve(additional).map_err(|_| ())
    }
    #[inline]
    fn mkr_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        self.mkr_reserve(additional)
    }
}

/// A `Vec<T>` with room for `cap` elements, or failure.
#[inline]
pub fn try_vec_with_capacity<T>(cap: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    if cap == 0 {
        return Some(v);
    }
    v.mkr_reserve_exact(cap).ok()?;
    Some(v)
}

/// Copy a slice into a fresh `Vec`, or fail.
#[inline]
pub fn try_to_vec<T: Clone>(s: &[T]) -> Option<Vec<T>> {
    let mut v = try_vec_with_capacity(s.len())?;
    v.extend_from_slice(s);
    Some(v)
}

/// Copy a slice into a fresh boxed slice, or fail. The shape most of the
/// pointer-keyed caches want for their keys.
#[inline]
pub fn try_to_boxed_slice<T: Clone>(s: &[T]) -> Option<Box<[T]>> {
    Some(try_to_vec(s)?.into_boxed_slice())
}

/// Inserting into a map, beyond the reserve itself.
pub trait MapInsert<K, V> {
    /// `Err(())` means the allocation failed and the map is unchanged.
    fn mkr_insert(&mut self, key: K, value: V) -> Result<(), ()>;
}

impl<K: core::hash::Hash + Eq, V, S: core::hash::BuildHasher> MapInsert<K, V> for HashMap<K, V, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn mkr_insert(&mut self, key: K, value: V) -> Result<(), ()> {
        self.mkr_reserve(1)?;
        self.insert(key, value);
        Ok(())
    }
}

/// Geometric growth for a hand-managed array, restated from the C
/// `mkr_grow_capacity`: double from 8 until it covers `need`, falling back to
/// exactly `need` when doubling would overshoot what `elem` allows. `None` only
/// when `need` itself does not fit.
///
/// This lives here rather than beside its one caller (`glue::node_set`) for a
/// reason worth keeping: that module needs magnus, hence a live Ruby, so
/// nothing in it can be built under Kani. The arithmetic is pure, and putting
/// it where it can be proved is the difference between a property that is
/// checked and one that is merely commented.
pub fn grow_capacity(cap: usize, need: usize, elem: usize) -> Option<usize> {
    need.checked_mul(elem)?;
    // A `cap` whose byte size does not fit cannot describe a live allocation,
    // so start over rather than hand it back. Kani found this: with a huge
    // `cap` and a small `need` the loop below never runs, and the function
    // returned a capacity that overflows on the next multiply. The one caller
    // only ever passes a real allocation size, so it was an unstated
    // precondition rather than a live bug - but this is a public helper now,
    // and an unstated precondition is the kind of thing that becomes a bug
    // when the second caller arrives.
    let start = match cap.checked_mul(elem) {
        Some(_) if cap != 0 => cap,
        _ => 8,
    };
    let mut nc = start;
    while nc < need {
        match nc.checked_mul(2) {
            Some(next) if next.checked_mul(elem).is_some() => nc = next,
            _ => return Some(need),
        }
    }
    Some(nc)
}

#[cfg(kani)]
mod verify;
