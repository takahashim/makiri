//! `RawVec`: a growable array of `Copy` elements over an allocator that is
//! only `realloc` and `free`.
//!
//! The bridge holds a NodeSet's nodes in Ruby's allocator (`ruby_xrealloc2`,
//! so the buffer is GC-accounted; `bridge::alloc::RubyVec`), which is why
//! this is a raw pointer, a length and a capacity rather than a `Vec`. The
//! pointer arithmetic is here, Ruby-free, generic over [`RawAlloc`], so the
//! same code the extension runs is what `kani` proves and Miri executes over
//! the standard allocator ([`tests`] and `verify`): only the two calls
//! into Ruby stay in the bridge.

#![allow(unsafe_code)]

use core::marker::PhantomData;

/// What [`RawVec`] needs of an allocator.
///
/// # Safety
/// An implementation keeps the contract each method states; [`RawVec`]
/// relies on it for every read and write it makes.
pub unsafe trait RawAlloc {
    /// Resize the block at `p` - null, or a live block from this allocator
    /// with room for `old` elements of `T` - to `new` (> 0) elements, keeping
    /// the first `old.min(new)`. `None` on failure, with the block untouched.
    ///
    /// # Safety
    /// `p` and `old` describe a block as above, or `p` is null and `old` 0.
    unsafe fn realloc<T>(p: *mut T, old: usize, new: usize) -> Option<*mut T>;

    /// Release the block at `p`, a live block from this allocator with room
    /// for `cap` elements; never called with null.
    ///
    /// # Safety
    /// `p` and `cap` describe a live block, freed once.
    unsafe fn free<T>(p: *mut T, cap: usize);
}

/// Why a [`RawVec`] could not grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowError {
    /// The next capacity does not fit the element size.
    Overflow,
    /// The allocator failed; the array is unchanged.
    NoMemory,
}

/// A growable array of `Copy` elements in `A`'s memory.
///
/// The invariant every method keeps: `ptr` is null exactly when `cap` is 0,
/// otherwise a live block of `A`'s with room for `cap` elements, of which
/// the first `len <= cap` are written.
pub struct RawVec<T: Copy, A: RawAlloc> {
    ptr: *mut T,
    len: usize,
    cap: usize,
    _a: PhantomData<A>,
}

impl<T: Copy, A: RawAlloc> Default for RawVec<T, A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, A: RawAlloc> RawVec<T, A> {
    /// An empty array, holding no allocation.
    pub const fn new() -> Self {
        RawVec {
            ptr: core::ptr::null_mut(),
            len: 0,
            cap: 0,
            _a: PhantomData,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The elements the allocation has room for.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn as_slice(&self) -> &[T] {
        if self.ptr.is_null() {
            return &[];
        }
        // SAFETY: the invariant: `ptr` is a live block of `cap >= len`
        // elements, the first `len` of them written.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Make room for `n` elements in one allocation, so the pushes that
    /// follow cannot reallocate.
    pub fn reserve(&mut self, n: usize) -> Result<(), GrowError> {
        if n > self.cap {
            self.grow_to(n)?;
        }
        Ok(())
    }

    /// Reallocate to exactly `cap` (> `self.cap`) elements.
    fn grow_to(&mut self, cap: usize) -> Result<(), GrowError> {
        if cap
            .checked_mul(core::mem::size_of::<T>())
            .is_none_or(|b| b > isize::MAX as usize)
        {
            return Err(GrowError::Overflow);
        }
        // SAFETY: the invariant: `ptr` is null with `cap` 0, or a live block
        // of `self.cap` elements; `cap > self.cap >= 0`.
        let p = unsafe { A::realloc(self.ptr, self.cap, cap) }.ok_or(GrowError::NoMemory)?;
        self.ptr = p;
        self.cap = cap;
        Ok(())
    }

    /// Append one element, growing geometrically (`falloc::grow_capacity`).
    pub fn push(&mut self, item: T) -> Result<(), GrowError> {
        if self.len == self.cap {
            let want = self.len.checked_add(1).ok_or(GrowError::Overflow)?;
            let new_cap = crate::falloc::grow_capacity(self.cap, want, core::mem::size_of::<T>())
                .ok_or(GrowError::Overflow)?;
            self.grow_to(new_cap)?;
        }
        // SAFETY: `len < cap` after the growth above, so the slot is inside
        // the live block.
        unsafe { self.ptr.add(self.len).write(item) };
        self.len += 1;
        Ok(())
    }
}

impl<T: Copy, A: RawAlloc> Drop for RawVec<T, A> {
    fn drop(&mut self) {
        crate::no_unwind::must_not_unwind::<crate::no_unwind::DropRubyVec, _>(|| {
            if !self.ptr.is_null() {
                // SAFETY: the invariant: a live block of `cap` elements, and
                // this is its owner's last use.
                unsafe { A::free(self.ptr, self.cap) };
            }
        })
    }
}

/// The standard allocator as a [`RawAlloc`], for the tests and proofs.
#[cfg(any(test, kani))]
pub struct StdAlloc;

#[cfg(any(test, kani))]
// SAFETY: `realloc` and `free` pair `std::alloc`'s calls with the layout of
// the block's element count, which the caller states.
unsafe impl RawAlloc for StdAlloc {
    unsafe fn realloc<T>(p: *mut T, old: usize, new: usize) -> Option<*mut T> {
        use std::alloc::Layout;
        let new_layout = Layout::array::<T>(new).ok()?;
        if new_layout.size() == 0 {
            return Some(core::ptr::NonNull::<T>::dangling().as_ptr());
        }
        // SAFETY: the caller's contract; a zero-sized old block is no
        // allocation, so it is allocated afresh.
        let q = unsafe {
            if p.is_null() || old == 0 || core::mem::size_of::<T>() == 0 {
                std::alloc::alloc(new_layout)
            } else {
                std::alloc::realloc(
                    p as *mut u8,
                    Layout::array::<T>(old).ok()?,
                    new_layout.size(),
                )
            }
        };
        (!q.is_null()).then_some(q as *mut T)
    }

    unsafe fn free<T>(p: *mut T, cap: usize) {
        if let Ok(layout) = std::alloc::Layout::array::<T>(cap) {
            if layout.size() != 0 {
                // SAFETY: the caller's contract.
                unsafe { std::alloc::dealloc(p as *mut u8, layout) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushes_and_reads_back_across_growth() {
        let mut v: RawVec<u64, StdAlloc> = RawVec::new();
        assert!(v.as_slice().is_empty());
        for i in 0..1000u64 {
            v.push(i).unwrap();
        }
        assert_eq!(v.len(), 1000);
        assert!(v.capacity() >= 1000);
        assert!(v.as_slice().iter().copied().eq(0..1000u64));
    }

    #[test]
    fn reserve_then_push_does_not_reallocate() {
        let mut v: RawVec<u32, StdAlloc> = RawVec::new();
        v.reserve(100).unwrap();
        let cap = v.capacity();
        for i in 0..100 {
            v.push(i).unwrap();
        }
        assert_eq!(v.capacity(), cap);
        v.reserve(10).unwrap();
        assert_eq!(v.capacity(), cap);
    }

    #[test]
    fn an_impossible_reserve_is_overflow_and_changes_nothing() {
        let mut v: RawVec<u64, StdAlloc> = RawVec::new();
        v.push(7).unwrap();
        assert_eq!(v.reserve(usize::MAX), Err(GrowError::Overflow));
        assert_eq!(v.as_slice(), &[7]);
    }

    #[test]
    fn zero_sized_elements() {
        let mut v: RawVec<(), StdAlloc> = RawVec::new();
        for _ in 0..10 {
            v.push(()).unwrap();
        }
        assert_eq!(v.len(), 10);
    }
}

#[cfg(kani)]
mod verify {
    use super::*;

    /// The standard allocator, failing whenever the solver chooses: so the
    /// proofs cover the failure path too (`StdAlloc` alone never fails).
    struct MaybeFail;

    // SAFETY: `StdAlloc`'s, or a failure that leaves the block untouched.
    unsafe impl RawAlloc for MaybeFail {
        unsafe fn realloc<T>(p: *mut T, old: usize, new: usize) -> Option<*mut T> {
            if kani::any() {
                return None;
            }
            // SAFETY: the caller's contract, passed on.
            unsafe { StdAlloc::realloc(p, old, new) }
        }

        unsafe fn free<T>(p: *mut T, cap: usize) {
            // SAFETY: as above.
            unsafe { StdAlloc::free(p, cap) }
        }
    }

    /// Any sequence of up to four pushes and reserves, each of which may find
    /// the allocator failing, keeps the array's contents what was pushed and
    /// accepted, in order, with every access in bounds and the block freed
    /// once.
    #[kani::proof]
    #[kani::unwind(6)]
    fn any_pushes_and_reserves_keep_the_contents() {
        let mut v: RawVec<u8, MaybeFail> = RawVec::new();
        let mut want = [0u8; 4];
        let mut n = 0usize;
        for _ in 0..4 {
            if kani::any() {
                let x: u8 = kani::any();
                let r = v.push(x);
                kani::cover!(r.is_err(), "a push the allocator refused");
                if r.is_ok() {
                    want[n] = x;
                    n += 1;
                }
            } else {
                let r: usize = kani::any();
                kani::assume(r <= 8);
                let _ = v.reserve(r);
            }
            assert!(v.len() <= v.capacity());
            assert_eq!(v.as_slice(), &want[..n]);
        }
        kani::cover!(n == 4, "four pushes accepted");
    }
}
