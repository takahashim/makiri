//! `Block`: `cap` initialised elements in an allocator that is only `realloc`
//! and `free` - the whole of `RawVec`'s `unsafe`.
//!
//! Every element of a block is initialised: a growth fills the new elements
//! with `T::default()`, so the block can always be seen as a `&[T]` and no
//! caller keeps a count of which elements are written. What the rest of
//! `RawVec` relies on is this module's one contract, the one Verus takes as
//! its axiom (`verus/raw_vec.rs`): a successful [`Block::resize`] leaves
//! `new` elements, the first `old.min(new)` unchanged; a failed one changes
//! nothing; [`Block::slots`] is the elements.

#![allow(unsafe_code)]

use core::marker::PhantomData;
use core::mem::MaybeUninit;

use super::GrowError;

/// What a [`Block`] needs of an allocator.
///
/// # Safety
/// An implementation keeps the contract each method states; [`Block`]
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

/// `cap` initialised elements of `T` in `A`'s memory.
///
/// The invariant: `ptr` is null exactly when `cap` is 0, otherwise a live
/// block of `A`'s with room for `cap` elements, every one initialised.
pub struct Block<T: Copy + Default, A: RawAlloc> {
    ptr: *mut T,
    cap: usize,
    _a: PhantomData<A>,
}

impl<T: Copy + Default, A: RawAlloc> Default for Block<T, A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Default, A: RawAlloc> Block<T, A> {
    /// A block of no elements, holding no allocation.
    pub const fn new() -> Self {
        Block {
            ptr: core::ptr::null_mut(),
            cap: 0,
            _a: PhantomData,
        }
    }

    /// The elements the block holds.
    #[inline]
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// The elements.
    pub fn slots(&self) -> &[T] {
        if self.ptr.is_null() {
            return &[];
        }
        // SAFETY: the invariant: a live block of `cap` initialised elements,
        // borrowed with `self`.
        unsafe { core::slice::from_raw_parts(self.ptr, self.cap) }
    }

    /// The elements, to write.
    pub fn slots_mut(&mut self) -> &mut [T] {
        if self.ptr.is_null() {
            return &mut [];
        }
        // SAFETY: as `slots`, borrowed mutably with `self`.
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.cap) }
    }

    /// Element `i` (< `cap`) set to `v`. Safe code over [`slots_mut`]: an
    /// index past the block is a panic, never a write outside it.
    ///
    /// [`slots_mut`]: Self::slots_mut
    #[inline]
    pub fn set(&mut self, i: usize, v: T) {
        self.slots_mut()[i] = v;
    }

    /// The first `n` (<= `cap`) elements. Safe code over [`slots`]: `n` past
    /// the block is a panic, never a read outside it.
    ///
    /// [`slots`]: Self::slots
    #[inline]
    pub fn prefix(&self, n: usize) -> &[T] {
        &self.slots()[..n]
    }

    /// Reallocate to exactly `new` (> `self.cap`) elements: the first
    /// `self.cap` kept, the rest `T::default()`. `Err` changes nothing.
    pub fn resize(&mut self, new: usize) -> Result<(), GrowError> {
        if new <= self.cap {
            return Err(GrowError::Overflow);
        }
        // SAFETY: the invariant: `ptr` is null with `cap` 0, or a live block
        // of `cap` elements; `new > cap >= 0`.
        let p = unsafe { A::realloc(self.ptr, self.cap, new) }.ok_or(GrowError::NoMemory)?;
        let old = self.cap;
        self.ptr = p;
        self.cap = new;
        // SAFETY: a live block of `new` elements, of which the first `old`
        // are initialised (`realloc` kept them) - and a `MaybeUninit` has no
        // validity requirement, so the whole block may be seen as one.
        let all = unsafe { core::slice::from_raw_parts_mut(p as *mut MaybeUninit<T>, new) };
        if let Some(tail) = all.get_mut(old..) {
            for slot in tail {
                slot.write(T::default());
            }
        }
        Ok(())
    }
}

impl<T: Copy + Default, A: RawAlloc> Drop for Block<T, A> {
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

/// The standard allocator, failing whenever Kani's solver chooses, so the
/// proofs cover the failure path too (`StdAlloc` alone never fails).
#[cfg(kani)]
pub struct MaybeFail;

#[cfg(kani)]
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
