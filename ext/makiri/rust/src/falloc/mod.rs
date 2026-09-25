//! Fallible allocation: the one place engine Rust code is allowed to ask for
//! memory.
//!
//! # Why this exists
//!
//! `rake oom` is the gate behind CLAUDE.md's fail-closed rule. It arms "the nth
//! allocation hook consultation fails", runs a workload, and asserts the result
//! is a clean exception or byte-identical to the baseline - never truncated,
//! never a crash.
//!
//! Raw `std` containers do not give us that:
//!
//!  1. **Not injectable.** Allocations made with `std` containers never consult
//!     the hook, so no sweep can reach them. Code that allocates through this
//!     module (or the libc facades in `cstr`) inherits it.
//!  2. **Not fallible.** `Box::new`, `Vec::push` and `HashMap::insert` abort the
//!     process on allocation failure (`handle_alloc_error`). For a library
//!     loaded into someone's application server, aborting is not "failing
//!     closed" - it takes the host down instead of raising.
//!
//! So this module does both: it consults the counter the sweep arms, and it
//! returns failure instead of aborting. Container growth goes through the
//! traits below, construction through the `try_*` free functions, and raw
//! NUL-terminated C strings through `cstr`. `clippy.toml` bans the `std`
//! methods these wrap, so a new call site cannot quietly go around the sweep.
//!
//! The `bridge`'s Ruby-side storage is the deliberate exception: it is Ruby's
//! `xmalloc` memory, freed by Ruby, and never reaches this counter.
//!
//! # The counter
//!
//! `inject` owns the single counter the sweep arms, and
//! `allocation_should_fail` reads it. One counter means one numbering: `rake
//! oom` does not need to know which code path owns consultation number 4,271,
//! and a sweep sized from a disarmed baseline run stays correct as sites move.
//!
//! It counts consultations, not allocations: `Reserve::falloc_reserve` consults
//! before a reserve that may not need to allocate. The sweep still covers every
//! real allocation, because the call sequence up to the armed index is exactly
//! the baseline one.
//!
//! The counter is atomic, not GVL-protected: a parse runs under
//! `rb_thread_call_without_gvl` and consults it from there, so two threads can
//! reach it at once.
//!
//! # Two shapes, and why
//!
//! Growth is on traits (`Reserve`, `VecPush`, `MapInsert`) because it has a
//! receiver: `v.falloc_push(x)` reads like the `v.push(x)` it replaces, which is
//! what kept the conversion of the call sites reviewable. Construction is free
//! functions (`try_box`, `try_vec_with_capacity`, `try_to_vec`, ...) because
//! there is nothing to hang a method on. Each names one shape and each has
//! callers; the split is by whether a receiver exists, not by accident.
//!
//! # Cost when not sweeping
//!
//! None. Without the `alloc-inject` feature `allocation_should_fail` is a
//! `const false` that the optimiser deletes along with the branch. extconf
//! turns the feature on for the same `MAKIRI_ALLOC_INJECT=1` that arms the
//! sweep.

#![allow(unsafe_code)]
// `Result<(), ()>` throughout: these report "the allocation failed", which
// carries no information beyond itself, and the crate already uses that shape
// for the same reason (see xml/chars.rs).
#![allow(clippy::result_unit_err)]

use std::collections::{HashMap, HashSet};

/// Its Kani proofs - the ownership contract at the raw boundaries, which is
/// what is left after the size arithmetic went to `checked_*` and the OOM
/// branches to `rake oom`.
pub mod calloc_verify;
pub(crate) mod cstr;
#[cfg(feature = "alloc-inject")]
pub(crate) mod inject;
/// The raw allocator primitives the Kani proofs quantify over. No production
/// path calls them any more; the typed API above and `cstr` cover every live
/// allocation.
#[cfg(kani)]
pub(crate) mod raw;

/* The injection counter has ONE home, `inject`. The allocator implementations
 * call only `allocation_should_fail`, which is a constant false in production. */

/* Re-exported rather than reached into directly so `init` (the Ruby test
 * bridge) does not have to know where the counter lives. `pub` for the same
 * reason `inject` keeps `pub` items: it must stay lint-clean in the
 * `alloc-inject`-without-`ruby` build, where nothing in-crate consumes it. */
#[cfg(feature = "alloc-inject")]
pub use inject::{alloc_inject_arm, alloc_inject_call_count};

#[cfg(feature = "alloc-inject")]
use inject::alloc_inject_should_fail;

/// Allocation instrumentation hook. Always false in production builds.
#[cfg(feature = "alloc-inject")]
#[inline(always)]
pub(crate) fn allocation_should_fail() -> bool {
    /* Safe to call from any thread: `inject` keeps the counter atomic, and a
     * parse consults it under `rb_thread_call_without_gvl`. The only contract
     * is the sweep's - call it once per allocation attempt - and violating it
     * mis-sizes the sweep rather than causing UB. */
    alloc_inject_should_fail()
}

/// Production allocator hook: no test instrumentation or branch remains.
#[cfg(not(feature = "alloc-inject"))]
#[inline(always)]
pub(crate) const fn allocation_should_fail() -> bool {
    false
}

/// `Box::new` that reports failure instead of aborting.
///
/// Written against the raw allocator rather than `Box::try_new` (unstable): ask
/// for the layout, write the value into it, and only then claim ownership. On
/// failure `value` is dropped and `Err(())` is returned, no allocation having
/// escaped. A zero-sized `T` never allocates, so it cannot fail and is not
/// counted.
#[inline]
pub fn try_box<T>(value: T) -> Result<Box<T>, ()> {
    let layout = std::alloc::Layout::new::<T>();
    if layout.size() == 0 {
        #[allow(clippy::disallowed_methods)]
        return Ok(Box::new(value));
    }
    if allocation_should_fail() {
        return Err(());
    }
    // SAFETY: the layout is non-zero-sized (checked above), so `alloc` is being
    // used within its contract. On success the pointer is fresh, uniquely
    // owned, aligned for T and uninitialised, which is exactly what `write`
    // needs and what `from_raw` then takes ownership of.
    unsafe {
        let p = std::alloc::alloc(layout) as *mut T;
        if p.is_null() {
            return Err(());
        }
        p.write(value);
        Ok(Box::from_raw(p))
    }
}

/// The fallible container operations, as one extension trait.
///
/// A trait rather than free functions because the call sites already read
/// `x.try_reserve(n).is_err()`; `x.falloc_reserve(n).is_err()` keeps that shape.
/// The `falloc_` prefix is deliberate: a method named `try_reserve` would be
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
    fn falloc_reserve(&mut self, additional: usize) -> Result<(), ()>;
    /// As `falloc_reserve`, without the growth slack. The hash containers below
    /// cannot honour the difference and forward to `falloc_reserve`.
    fn falloc_reserve_exact(&mut self, additional: usize) -> Result<(), ()>;
}

/// Growing a `Vec`, beyond the reserve itself.
pub trait VecPush<T> {
    /// Push one element. `Err(())` leaves the vector unchanged.
    fn falloc_push(&mut self, item: T) -> Result<(), ()>;
    /// Push one element, asking the allocator - and so the injection counter -
    /// only when the vector is full, and then growing geometrically
    /// ([`grow_capacity`]). For a push per node of a walk: [`falloc_push`]
    /// consults the counter on every call, which made each push its own
    /// `rake oom` injection point re-testing one branch. `Err(())` leaves the
    /// vector unchanged.
    ///
    /// [`falloc_push`]: VecPush::falloc_push
    fn falloc_push_amortized(&mut self, item: T) -> Result<(), ()>;
    /// Append a slice. `Err(())` leaves the vector unchanged.
    fn falloc_extend(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Clone;
}

impl<T> VecPush<T> for Vec<T> {
    #[inline]
    fn falloc_push(&mut self, item: T) -> Result<(), ()> {
        self.falloc_reserve(1)?;
        self.push(item);
        Ok(())
    }
    #[inline]
    fn falloc_push_amortized(&mut self, item: T) -> Result<(), ()> {
        if self.len() == self.capacity() {
            let want = grow_capacity(self.capacity(), self.len() + 1, core::mem::size_of::<T>())
                .ok_or(())?;
            self.falloc_reserve_exact(want - self.len())?;
        }
        self.push(item);
        Ok(())
    }
    #[inline]
    fn falloc_extend(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Clone,
    {
        self.falloc_reserve(s.len())?;
        self.extend_from_slice(s);
        Ok(())
    }
}

/// The one shape of every reserve here: ask the injection hook first (so
/// `rake oom` can fail this site), then std's fallible reserve.
#[inline]
fn injectable<E>(reserve: impl FnOnce() -> Result<(), E>) -> Result<(), ()> {
    if allocation_should_fail() {
        return Err(());
    }
    reserve().map_err(|_| ())
}

impl<T> Reserve for Vec<T> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve(&mut self, additional: usize) -> Result<(), ()> {
        injectable(|| self.try_reserve(additional))
    }
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        injectable(|| self.try_reserve_exact(additional))
    }
}

impl<K: core::hash::Hash + Eq, V, S: core::hash::BuildHasher> Reserve for HashMap<K, V, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve(&mut self, additional: usize) -> Result<(), ()> {
        injectable(|| self.try_reserve(additional))
    }
    #[inline]
    fn falloc_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        self.falloc_reserve(additional)
    }
}

impl<T: core::hash::Hash + Eq, S: core::hash::BuildHasher> Reserve for HashSet<T, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve(&mut self, additional: usize) -> Result<(), ()> {
        injectable(|| self.try_reserve(additional))
    }
    #[inline]
    fn falloc_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        self.falloc_reserve(additional)
    }
}

/// A `Vec<T>` with room for `cap` elements, or failure.
#[inline]
pub fn try_vec_with_capacity<T>(cap: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    if cap == 0 {
        return Some(v);
    }
    v.falloc_reserve_exact(cap).ok()?;
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
    fn falloc_insert(&mut self, key: K, value: V) -> Result<(), ()>;
}

impl<K: core::hash::Hash + Eq, V, S: core::hash::BuildHasher> MapInsert<K, V> for HashMap<K, V, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_insert(&mut self, key: K, value: V) -> Result<(), ()> {
        self.falloc_reserve(1)?;
        self.insert(key, value);
        Ok(())
    }
}

/// Geometric growth for a hand-managed array: start from the current capacity
/// (or 8 when there is none and 8 elements fit), double until it covers `need`,
/// and fall back to exactly `need` when doubling would overshoot what `elem`
/// allows. `None` only when `need` itself does not fit.
///
/// This lives here rather than beside its callers (`cbuf`, the text index, the
/// proofs) for a reason worth keeping: the arithmetic is pure, so it can be
/// built under Kani and `cargo test` without Lexbor or Ruby. Putting it where it
/// can be proved is the difference between a property that is checked and one
/// that is merely commented.
pub fn grow_capacity(cap: usize, need: usize, elem: usize) -> Option<usize> {
    need.checked_mul(elem)?;
    // No allocation is required for an empty request. More importantly, do
    // not manufacture the usual initial capacity (8) here: for an arbitrary
    // element size that capacity may itself be unallocatable even though zero
    // elements fit. The public helper's contract is about every `elem`, not
    // only its current pointer-sized caller.
    if need == 0 {
        return Some(0);
    }
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
        // The usual initial capacity is an optimisation, never a contract.
        // If eight elements do not fit, `need` is already known to fit and is
        // the only valid starting point.
        _ if 8usize.checked_mul(elem).is_some() => 8,
        _ => need,
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
