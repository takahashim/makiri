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
//!     module inherits it.
//!  2. **Not fallible.** `Box::new`, `Vec::push` and `HashMap::insert` abort the
//!     process on allocation failure (`handle_alloc_error`). For a library
//!     loaded into someone's application server, aborting is not "failing
//!     closed" - it takes the host down instead of raising.
//!
//! So this module does both: it consults the counter the sweep arms, and it
//! returns failure instead of aborting. Container growth goes through the
//! traits below, construction through the `try_*` free functions.
//! `clippy.toml` bans the `std` methods these wrap, so a new call site cannot
//! quietly go around the sweep.
//!
//! The `bridge`'s Ruby-side storage is the deliberate exception: a TypedData
//! wrapper's data (`rb_data_typed_object_zalloc`) is Ruby's memory, freed by
//! Ruby; it never reaches this counter, and `rake oom`'s raise sweep fails it
//! instead. A NodeSet's node array is this crate's (a `Vec` grown here),
//! reported to Ruby's GC as it grows.
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
//! Growth is on traits (`Reserve`, `VecGrow`, `MapInsert`) because it has a
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

// The one `unsafe` is `try_box`'s, which carries the only `allow`: a
// fallible `Box<T>` has no stable safe spelling. It is a crate of its own so
// the engine that allocates through it can forbid `unsafe` outright.
#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
// `Result<(), ()>` throughout: these report "the allocation failed", which
// carries no information beyond itself, and the crate already uses that shape
// for the same reason (see xml/chars.rs).
#![allow(clippy::result_unit_err)]

use std::collections::{HashMap, HashSet};

#[cfg(feature = "alloc-inject")]
pub(crate) mod inject;

/* The injection counter has ONE home, `inject`. The allocator implementations
 * call only `allocation_should_fail`, which is a constant false in production. */

/* Re-exported rather than reached into directly so `init` (the Ruby test
 * bridge) does not have to know where the counter lives. `pub` for the same
 * reason `inject` keeps `pub` items: it must stay lint-clean in the
 * `alloc-inject`-without-`ruby` build, where nothing in-crate consumes it. */
#[cfg(feature = "alloc-inject")]
pub use inject::{
    alloc_inject_arm, alloc_inject_call_count, raise_inject_arm, raise_inject_call_count,
    raise_inject_should_raise,
};

#[cfg(feature = "alloc-inject")]
use inject::alloc_inject_should_fail;

/// Allocation instrumentation hook. Always false in production builds.
#[cfg(feature = "alloc-inject")]
#[inline(always)]
pub fn allocation_should_fail() -> bool {
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
#[allow(unsafe_code)]
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

/// Growing a `Vec`, beyond the reserve itself: every std method that grows
/// one, spelled so that the growth is a falloc reserve. `clippy.toml` bans the
/// std spellings, so "reserve exactly what follows, then the plain call" - a
/// count that review alone checked, and a miscount that grew outside the
/// sweep - is no longer a shape a call site can take.
///
/// Each reserves first, and the injection hook is asked only when that
/// reserve has to grow (see [`injectable`]), so a call into room already
/// reserved costs one comparison. `Err(())` leaves the vector unchanged,
/// [`falloc_extend`](Self::falloc_extend) included. The copying ones take
/// `T: Copy` for [`try_to_vec`]'s reason: a `Clone` element's `clone` may
/// allocate outside the sweep.
pub trait VecGrow<T> {
    /// Push one element, growing geometrically (std's amortized growth) when
    /// full. The allocator - and so the injection hook - is asked only then,
    /// so a push per node of a walk is one injection point per growth.
    fn falloc_push(&mut self, item: T) -> Result<(), ()>;
    /// `insert(index, item)`. Panics, as `insert` does, past the length.
    fn falloc_insert(&mut self, index: usize, item: T) -> Result<(), ()>;
    /// `extend_from_slice`.
    fn falloc_extend_from_slice(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Copy;
    /// `extend_from_within(range)`. Panics, as it does, on a range out of bounds.
    fn falloc_extend_from_within(&mut self, range: core::ops::Range<usize>) -> Result<(), ()>
    where
        T: Copy;
    /// `resize(new_len, value)`; a shrink allocates nothing.
    fn falloc_resize(&mut self, new_len: usize, value: T) -> Result<(), ()>
    where
        T: Copy;
    /// `extend(iter)`: one reserve for the iterator's lower bound, then a
    /// push per element; on a failure the elements already pushed are
    /// removed, so the vector is as it was.
    fn falloc_extend<I: IntoIterator<Item = T>>(&mut self, iter: I) -> Result<(), ()>;
    /// Room for one element, reserved now and filled later by
    /// [`Slot::push`], which cannot fail - see [`Slot`].
    fn falloc_slot(&mut self) -> Result<Slot<'_, T>, ()>;
}

/// One element's room in a `Vec`, reserved: the guarantee that a later push
/// will not grow it, in a type.
///
/// For an operation that changes several things and must change all or
/// none: take every slot first - each a fallible reserve - and then fill
/// them, which cannot fail, so a failure always comes before anything has
/// changed. The slot borrows the vector until it is filled, so nothing can
/// take the room in between, and [`push`](Slot::push) consumes it, so it
/// fills one element and no more. (The shape of std's `VacantEntry`.) A slot
/// dropped unfilled leaves the vector as it was, with the room to spare.
///
/// What the compiler holds it to. Nothing else can use the vector while a
/// slot is out - so nothing can take its room:
///
/// ```compile_fail,E0499
/// use falloc::VecGrow;
/// let mut v: Vec<u8> = Vec::new();
/// let slot = v.falloc_slot().unwrap();
/// v.falloc_push(1).unwrap();
/// slot.push(2);
/// ```
///
/// and a slot fills one element, once:
///
/// ```compile_fail,E0382
/// use falloc::VecGrow;
/// let mut v: Vec<u8> = Vec::new();
/// let slot = v.falloc_slot().unwrap();
/// slot.push(1);
/// slot.push(2);
/// ```
#[must_use = "a slot reserves room for one push; fill it with `push`"]
pub struct Slot<'a, T>(&'a mut Vec<T>);

impl<T> Slot<'_, T> {
    /// The index the element will have.
    #[inline]
    pub fn index(&self) -> usize {
        self.0.len()
    }

    /// Fill the slot. Allocates nothing: the room was reserved when the slot
    /// was made, and nothing could use it since.
    #[inline]
    #[allow(
        clippy::disallowed_methods,
        reason = "the slot's room was reserved when it was made"
    )]
    pub fn push(self, item: T) {
        debug_assert!(self.0.len() < self.0.capacity());
        self.0.push(item);
    }
}

#[allow(clippy::disallowed_methods)]
impl<T> VecGrow<T> for Vec<T> {
    #[inline]
    fn falloc_push(&mut self, item: T) -> Result<(), ()> {
        self.falloc_reserve(1)?;
        self.push(item);
        Ok(())
    }
    #[inline]
    fn falloc_insert(&mut self, index: usize, item: T) -> Result<(), ()> {
        self.falloc_reserve(1)?;
        self.insert(index, item);
        Ok(())
    }
    #[inline]
    fn falloc_extend_from_slice(&mut self, s: &[T]) -> Result<(), ()>
    where
        T: Copy,
    {
        self.falloc_reserve(s.len())?;
        self.extend_from_slice(s);
        Ok(())
    }
    #[inline]
    fn falloc_extend_from_within(&mut self, range: core::ops::Range<usize>) -> Result<(), ()>
    where
        T: Copy,
    {
        self.falloc_reserve(range.end.saturating_sub(range.start))?;
        self.extend_from_within(range);
        Ok(())
    }
    #[inline]
    fn falloc_resize(&mut self, new_len: usize, value: T) -> Result<(), ()>
    where
        T: Copy,
    {
        self.falloc_reserve(new_len.saturating_sub(self.len()))?;
        self.resize(new_len, value);
        Ok(())
    }
    #[inline]
    fn falloc_slot(&mut self) -> Result<Slot<'_, T>, ()> {
        self.falloc_reserve(1)?;
        Ok(Slot(self))
    }
    #[inline]
    fn falloc_extend<I: IntoIterator<Item = T>>(&mut self, iter: I) -> Result<(), ()> {
        let iter = iter.into_iter();
        let start = self.len();
        self.falloc_reserve(iter.size_hint().0)?;
        for item in iter {
            if self.falloc_push(item).is_err() {
                self.truncate(start);
                return Err(());
            }
        }
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

/// `vec![value; len]`, or failure: the shape of an index sized to an arena.
#[inline]
pub fn try_vec_filled<T: Copy>(len: usize, value: T) -> Option<Vec<T>> {
    let mut v = try_vec_with_capacity(len)?;
    v.falloc_resize(len, value).ok()?;
    Some(v)
}

/// The concatenation of `parts`, in one allocation of exactly their total
/// length, or failure: a qualified name from its prefix, `:` and local name,
/// an index key from its pieces.
pub fn try_concat<T: Copy>(parts: &[&[T]]) -> Option<Vec<T>> {
    let len = parts
        .iter()
        .try_fold(0usize, |n, p| n.checked_add(p.len()))?;
    let mut v = try_vec_with_capacity(len)?;
    for p in parts {
        v.falloc_extend_from_slice(p).ok()?;
    }
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
    v.falloc_extend_from_slice(s).ok()?;
    Some(v)
}

/// Copy a slice into a fresh boxed slice, or fail. The shape most of the
/// pointer-keyed caches want for their keys. `T: Copy` for the same reason as
/// [`try_to_vec`].
///
/// Built through a `Vec` reserved to exactly `s.len()`, and handed to
/// `into_boxed_slice` only when its capacity IS its length. That call shrinks
/// a vector with spare room - a reallocation that aborts on OOM, outside the
/// injection counter - so it must never find any; std records the exact
/// requested capacity today, and should it ever round up, this answers `None`
/// (fail closed) rather than reallocating. A zero-sized `T`'s vector never
/// allocates (its capacity reads as `usize::MAX`), so it needs no check; an
/// empty slice or a zero-sized `T` is not counted.
#[inline]
pub fn try_to_boxed_slice<T: Copy>(s: &[T]) -> Option<Box<[T]>> {
    let mut v = Vec::new();
    v.falloc_reserve_exact(s.len()).ok()?;
    v.falloc_extend_from_slice(s).ok()?;
    if core::mem::size_of::<T>() != 0 && v.capacity() != v.len() {
        return None;
    }
    /* The one sanctioned call: the check above leaves it nothing to shrink. */
    #[allow(clippy::disallowed_methods)]
    Some(v.into_boxed_slice())
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

/// Inserting into a set: [`MapInsert`]'s shape, with `true` when the value
/// was new (std's `insert`).
pub trait SetInsert<T> {
    fn falloc_insert(&mut self, value: T) -> Result<bool, ()>;
}

impl<T: core::hash::Hash + Eq, S: core::hash::BuildHasher> SetInsert<T> for HashSet<T, S> {
    #[inline]
    #[allow(clippy::disallowed_methods)]
    fn falloc_insert(&mut self, value: T) -> Result<bool, ()> {
        /* Unconditional, as in `MapInsert`: std reserves before it looks. */
        self.falloc_reserve(1)?;
        Ok(self.insert(value))
    }
}

/// A domain error with one variant meaning "an allocation failed" - the
/// target of [`OomResult::or_oom`] / [`OomOption::or_oom`] below.
///
/// One `impl` per error enum, each naming its own variant. NOT for an enum
/// whose allocation failures are not all the same case (`SelectError` has
/// three - `CacheOom`/`CollectOom`/`ParseOom`, each meaning something
/// different to the caller): forcing those through one generic conversion
/// would erase which one happened, so they stay spelled out at the call site.
pub trait Oom {
    fn oom() -> Self;
}

/// `Result<T, ()>` from a growth op ([`Reserve`], [`VecGrow`], [`MapInsert`])
/// -> a domain `Result<T, E>`, naming only which error type - not which
/// variant, since [`Oom::oom`] already knows that.
///
/// This, [`OomOption`] and the local `impl Oom` per error type replace three
/// styles that had accumulated at call sites for the same conversion -
/// `.map_err(|_| E::Oom)`, `.ok_or(E::Oom)`/`.ok_or_else(...)`, and a manual
/// `if ... .is_err() { return Err(E::Oom); }` (sometimes two such calls
/// joined by `||`, which reads as one check but is really "try the first,
/// and only the first's failure decides whether the second runs at all" -
/// `.or_oom()?` on each, in sequence, says that directly).
pub trait OomResult<T> {
    fn or_oom<E: Oom>(self) -> Result<T, E>;
}

impl<T> OomResult<T> for Result<T, ()> {
    #[inline]
    fn or_oom<E: Oom>(self) -> Result<T, E> {
        self.map_err(|()| E::oom())
    }
}

/// `Option<T>` from a construction op (`try_vec_with_capacity`, `try_to_vec`,
/// `try_to_boxed_slice`, `try_box`) -> a domain `Result<T, E>`. See
/// [`OomResult`].
pub trait OomOption<T> {
    fn or_oom<E: Oom>(self) -> Result<T, E>;
}

impl<T> OomOption<T> for Option<T> {
    #[inline]
    fn or_oom<E: Oom>(self) -> Result<T, E> {
        self.ok_or_else(E::oom)
    }
}

/// Geometric growth for a hand-managed array: start from the current capacity
/// (or 8 when there is none and 8 elements fit), double until it covers `need`,
/// and fall back to exactly `need` when doubling would overshoot what `elem`
/// allows. `None` only when `need` itself does not fit.
///
/// This lives here rather than beside its callers (`ceiling_buf`,
/// `bridge::node_set`, the proofs) for a reason worth keeping: the arithmetic
/// is pure, so it can be proved (`verus/ceiling_buf.rs`, at every element size) and
/// tested without Lexbor or Ruby. Putting it where it can be proved is the difference between a property
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

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] /* test inputs */
    use super::{try_to_boxed_slice, VecGrow};

    /// A slot's push uses the room the slot reserved - the capacity does not
    /// move - and a slot dropped unfilled changes nothing but the room.
    #[test]
    fn a_slot_fills_its_reserved_room() {
        let mut v: Vec<u32> = Vec::new();
        let slot = v.falloc_slot().expect("slot");
        assert_eq!(slot.index(), 0);
        drop(slot);
        assert!(v.is_empty());
        let cap = v.capacity();
        assert!(cap >= 1);
        for i in 0..cap {
            let slot = v.falloc_slot().expect("slot");
            assert_eq!(slot.index(), i);
            slot.push(i as u32);
            assert_eq!(
                v.capacity(),
                cap,
                "a push into reserved room grew the vector"
            );
        }
        assert_eq!(v, (0..cap as u32).collect::<Vec<_>>());
    }

    /// The construction must preserve length and bytes for a
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

/// `nupur::utf8_copy`, the checked copy behind the extension's views of a Ruby
/// String, included here in the test build: it depends on std alone and holds
/// an `unsafe`, and Miri runs its tests in this crate (`rake miri`), which
/// nupur, built over rb-sys, cannot be. This is the crate where an `unsafe`
/// may be allowed.
#[cfg(test)]
#[path = "../../nupur/src/utf8_copy.rs"]
#[allow(dead_code, reason = "included for its tests; the API is nupur's")]
mod utf8_copy;
