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
//! It counts allocation ATTEMPTS, not calls: `injectable` asks the hook only
//! when a reserve would actually allocate, so a push per node of a walk costs
//! the sweep one injection point per growth rather than one per push. The sweep
//! still covers every real allocation, because the consult sequence up to the
//! armed index is exactly the baseline one.
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

pub(crate) mod cstr;
#[cfg(feature = "alloc-inject")]
pub(crate) mod inject;

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
    /// Push one element, growing geometrically (std's amortized growth) when
    /// full. The allocator - and so the injection hook - is asked only then,
    /// so a push per node of a walk is one injection point per growth.
    /// `Err(())` leaves the vector unchanged.
    fn falloc_push(&mut self, item: T) -> Result<(), ()>;
    /// Append a slice. `Err(())` leaves the vector unchanged.
    ///
    /// `T: Copy`, not `Clone`: `extend_from_slice` clones each element, and a
    /// non-`Copy` element's `clone` can allocate through the global allocator,
    /// outside the sweep and aborting on failure. `Copy`'s clone is a bitwise
    /// copy, so the reserve above is the only allocation.
    fn falloc_extend(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Copy;
}

impl<T> VecPush<T> for Vec<T> {
    #[inline]
    fn falloc_push(&mut self, item: T) -> Result<(), ()> {
        self.falloc_reserve(1)?;
        self.push(item);
        Ok(())
    }
    #[inline]
    fn falloc_extend(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Copy,
    {
        self.falloc_reserve(s.len())?;
        self.extend_from_slice(s);
        Ok(())
    }
}

/// The one shape of every reserve here: when the `spare` room already covers
/// `additional`, nothing is allocated and the hook is not asked; otherwise ask
/// the injection hook (so `rake oom` can fail this site), then std's fallible
/// reserve.
///
/// Skipping the hook when there is room is the hook's own contract - consult
/// it once per allocation ATTEMPT - and what makes a push per node of a walk
/// cost the sweep one injection point per growth rather than one per push.
#[inline]
fn injectable<E>(
    spare: usize,
    additional: usize,
    reserve: impl FnOnce() -> Result<(), E>,
) -> Result<(), ()> {
    if spare >= additional {
        return Ok(());
    }
    if allocation_should_fail() {
        return Err(());
    }
    reserve().map_err(|_| ())
}

impl<T> Reserve for Vec<T> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve(&mut self, additional: usize) -> Result<(), ()> {
        injectable(self.capacity() - self.len(), additional, || {
            self.try_reserve(additional)
        })
    }
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve_exact(&mut self, additional: usize) -> Result<(), ()> {
        injectable(self.capacity() - self.len(), additional, || {
            self.try_reserve_exact(additional)
        })
    }
}

impl<K: core::hash::Hash + Eq, V, S: core::hash::BuildHasher> Reserve for HashMap<K, V, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_reserve(&mut self, additional: usize) -> Result<(), ()> {
        injectable(self.capacity() - self.len(), additional, || {
            self.try_reserve(additional)
        })
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
        injectable(self.capacity() - self.len(), additional, || {
            self.try_reserve(additional)
        })
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
    v.falloc_reserve_exact(cap).ok()?;
    Some(v)
}

/// Copy a slice into a fresh `Vec`, or fail.
///
/// `T: Copy`, not `Clone`: this is the elementwise copy path, and a non-`Copy`
/// element's `clone` may allocate through the global allocator - outside the
/// sweep and aborting on failure - under a name that promises fallible
/// allocation. Every caller copies bytes or handle words.
#[inline]
pub fn try_to_vec<T: Copy>(s: &[T]) -> Option<Vec<T>> {
    let mut v = try_vec_with_capacity(s.len())?;
    v.extend_from_slice(s);
    Some(v)
}

/// Copy a slice into a fresh boxed slice, or fail. The shape most of the
/// pointer-keyed caches want for their keys. `T: Copy` for the same reason as
/// [`try_to_vec`].
///
/// Built against the raw allocator like [`try_box`], NOT through
/// `Vec::into_boxed_slice`: that calls `shrink_to_fit`, which reallocates and
/// aborts on OOM when the vector's capacity exceeds its length - outside the
/// injection counter, and safe today only because std happens to record the
/// exact requested capacity. Asking for `Layout::array` directly removes that
/// dependence on a std implementation detail. A zero-sized layout (an empty
/// slice, or a zero-sized `T`) never allocates and is not counted.
#[inline]
pub fn try_to_boxed_slice<T: Copy>(s: &[T]) -> Option<Box<[T]>> {
    let layout = std::alloc::Layout::array::<T>(s.len()).ok()?;
    if layout.size() == 0 {
        // SAFETY: a zero-sized element needs no allocation; the dangling,
        // correctly aligned pointer is the valid representation of a boxed
        // slice of zero-sized elements, and `Box` does not deallocate a
        // zero-sized layout.
        return Some(unsafe {
            Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                std::ptr::NonNull::<T>::dangling().as_ptr(),
                s.len(),
            ))
        });
    }
    if allocation_should_fail() {
        return None;
    }
    // SAFETY: `layout` is non-zero-sized (checked above) and describes exactly
    // `s.len()` elements of `T`, so `alloc` is within its contract. On success
    // the pointer is fresh, uniquely owned, aligned for T and uninitialised,
    // which is what the copy and the ownership claim below need; `T: Copy`, so
    // the bitwise copy is a valid clone.
    unsafe {
        let p = std::alloc::alloc(layout) as *mut T;
        if p.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
        Some(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            p,
            s.len(),
        )))
    }
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
        // `std`'s `HashMap::insert` calls `reserve(1)` BEFORE it looks the key
        // up, so a replace grows the table too when it is full (measured: a
        // len == capacity == 3 table becomes 7 on an existing-key insert, and
        // HashSet the same). So this reserve is a real allocation attempt even
        // for a replace, and must stay unconditional: skipping it because the
        // key exists would perform that allocation outside the injection
        // counter, where an OOM aborts the process instead of failing closed.
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
/// This lives here rather than beside its callers (`cbuf`,
/// `bridge::node_set`, the proofs) for a reason worth keeping: the arithmetic
/// is pure, so it can be built under Kani and `cargo test` without Lexbor or
/// Ruby. Putting it where it can be proved is the difference between a property
/// that is checked and one that is merely commented.
pub fn grow_capacity(cap: usize, need: usize, elem: usize) -> Option<usize> {
    need.checked_mul(elem)?;
    // No allocation is required for an empty request. Answer the CURRENT
    // capacity rather than 0: a caller that passed 0 on to `realloc` would free
    // the block (glibc), turning a harmless no-op into a use-after-free or a
    // double free. A `cap` that cannot describe a live allocation - zero, or a
    // byte size that does not fit - has nothing to keep, so 0 is right there.
    if need == 0 {
        return Some(if cap != 0 && cap.checked_mul(elem).is_some() {
            cap
        } else {
            0
        });
    }
    // A `cap` whose byte size does not fit cannot describe a live allocation,
    // so start over rather than hand it back. Kani found this: with a huge
    // `cap` and a small `need` the loop below never runs, and the function
    // returned a capacity that overflows on the next multiply. The callers only
    // ever pass a real allocation size, so it was an unstated precondition
    // rather than a live bug - but this is a public helper, and the check makes
    // the precondition explicit instead of leaving it to whoever calls next.
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

#[cfg(test)]
mod tests {
    use super::try_to_boxed_slice;

    /// The raw-allocator construction must preserve length and bytes for a
    /// normal element, an empty slice, and a zero-sized element of non-zero
    /// length (the three shapes the pointer arithmetic in it can take).
    #[test]
    fn boxed_slice_copies_exactly() {
        let src = [1u8, 2, 3, 4];
        let b = try_to_boxed_slice(&src).expect("boxed");
        assert_eq!(&b[..], &src[..]);

        let empty: [u8; 0] = [];
        let b = try_to_boxed_slice(&empty).expect("boxed empty");
        assert!(b.is_empty());

        let zsts = [(); 3];
        let b = try_to_boxed_slice(&zsts).expect("boxed zst");
        assert_eq!(b.len(), 3);
    }
}
