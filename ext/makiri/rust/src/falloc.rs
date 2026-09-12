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

#[cfg(feature = "alloc-inject")]
extern "C" {
    /// `core/mkr_alloc.c`. Counts every attempt (armed or not) so the harness
    /// can size its sweep from a disarmed run, and fails exactly one.
    fn mkr_alloc_inject_should_fail() -> core::ffi::c_int;
}

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

/// The reserve family, as an extension trait.
///
/// A trait rather than free functions because the call sites already read
/// `x.try_reserve(n).is_err()`; `x.mkr_reserve(n).is_err()` keeps that shape, so
/// the diff that made the crate injectable is a one-word change per site and
/// stays reviewable. The `mkr_` prefix is deliberate: a method named
/// `try_reserve` would be shadowed by the inherent one silently, which is
/// exactly the bug this module exists to prevent.
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

/// Push one element. False means the allocation failed and `v` is unchanged.
#[inline]
pub fn try_push<T>(v: &mut Vec<T>, item: T) -> bool {
    if v.mkr_reserve(1).is_err() {
        return false;
    }
    v.push(item);
    true
}

/// Append a slice. False means the allocation failed and `v` is unchanged.
#[inline]
pub fn try_extend_from_slice<T: Clone>(v: &mut Vec<T>, s: &[T]) -> bool {
    if v.mkr_reserve(s.len()).is_err() {
        return false;
    }
    v.extend_from_slice(s);
    true
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

/// Insert one entry. False means the allocation failed and `m` is unchanged.
///
/// The reserve is what can fail; the insert that follows cannot, because the
/// room is already there. A key that is already present replaces in place and
/// the spare capacity stays for the next insert, which is harmless.
#[inline]
#[allow(clippy::disallowed_methods)]
pub fn try_map_insert<K: core::hash::Hash + Eq, V, S: core::hash::BuildHasher>(
    m: &mut HashMap<K, V, S>,
    key: K,
    value: V,
) -> bool {
    if m.mkr_reserve(1).is_err() {
        return false;
    }
    m.insert(key, value);
    true
}
