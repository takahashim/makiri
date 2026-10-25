//! `RawVec`: a growable array of `Copy` elements over an allocator that is
//! only `realloc` and `free`.
//!
//! The bridge holds a NodeSet's nodes in Ruby's allocator (`ruby_xrealloc2`,
//! so the buffer is GC-accounted; `bridge::alloc::RubyVec`), which is why
//! this is not a `Vec`. The pointer is [`Block`]'s alone (`block.rs`, this
//! module's only `unsafe`): `cap` elements, all initialised, seen as a
//! slice. `RawVec` is safe code over it - a length, and growth by
//! `falloc::grow_capacity` - which `verus/raw_vec.rs` proves, copied token
//! for token (`rake verus:sync`), with `Block`'s contract as its axiom; Kani
//! (`verify.rs`) and Miri run it over the standard allocator.

mod block;

#[cfg(any(test, kani))]
pub use block::StdAlloc;
pub use block::{Block, RawAlloc};
#[cfg(kani)]
mod verify;

/// Why a [`RawVec`] could not grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowError {
    /// The next capacity does not fit the element size.
    Overflow,
    /// The allocator failed; the array is unchanged.
    NoMemory,
}

/// A growable array of `Copy` elements in `A`'s memory: the first `len` of
/// a [`Block`]'s elements.
pub struct RawVec<T: Copy + Default, A: RawAlloc> {
    block: Block<T, A>,
    len: usize,
}

impl<T: Copy + Default, A: RawAlloc> Default for RawVec<T, A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Default, A: RawAlloc> RawVec<T, A> {
    /// An empty array, holding no allocation.
    pub const fn new() -> Self {
        RawVec {
            block: Block::new(),
            len: 0,
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
        self.block.cap()
    }

    pub fn as_slice(&self) -> &[T] {
        self.block.prefix(self.len)
    }

    /// Make room for `n` elements in one allocation, so the pushes that
    /// follow cannot reallocate.
    pub fn reserve(&mut self, n: usize) -> Result<(), GrowError> {
        if n > self.block.cap() {
            self.grow_to(n)?;
        }
        Ok(())
    }

    /// Reallocate to exactly `cap` (> the capacity) elements, whose bytes
    /// must fit an `isize`.
    fn grow_to(&mut self, cap: usize) -> Result<(), GrowError> {
        let fits = match cap.checked_mul(core::mem::size_of::<T>()) {
            Some(bytes) => bytes <= isize::MAX as usize,
            None => false,
        };
        if !fits {
            return Err(GrowError::Overflow);
        }
        self.block.resize(cap)
    }

    /// Append one element, growing geometrically (`falloc::grow_capacity`).
    pub fn push(&mut self, item: T) -> Result<(), GrowError> {
        if self.len == self.block.cap() {
            let want = match self.len.checked_add(1) {
                Some(w) => w,
                None => return Err(GrowError::Overflow),
            };
            let new_cap = match crate::falloc::grow_capacity(
                self.block.cap(),
                want,
                core::mem::size_of::<T>(),
            ) {
                Some(c) => c,
                None => return Err(GrowError::Overflow),
            };
            self.grow_to(new_cap)?;
        }
        self.block.set(self.len, item);
        self.len += 1;
        Ok(())
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

    #[test]
    fn growth_fills_the_new_elements_with_the_default() {
        let mut b: Block<u32, StdAlloc> = Block::new();
        b.resize(3).unwrap();
        b.set(0, 7);
        b.resize(5).unwrap();
        assert_eq!(b.slots(), &[7, 0, 0, 0, 0]);
    }
}
