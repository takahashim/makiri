//! Ruby's allocator, for the few buffers Ruby's GC owns directly.
//!
//! Ruby hands these pointers back to `ruby_xfree` itself through a TypedData
//! free callback, so the memory must come from the same allocator. Everything
//! else in the extension allocates through `falloc`; these two are for the
//! structures Ruby's GC frees.

#![allow(unsafe_code)]

use core::ffi::c_void;

/// `ruby_xfree`: release a block from Ruby's allocator.
///
/// # Safety
/// `p` must be null or a live block from Ruby's allocator, freed once.
#[inline]
pub unsafe fn free<T>(p: *mut T) {
    // SAFETY: the caller's contract.
    unsafe { rb_sys::ruby_xfree(p as *mut c_void) };
}

/// `ruby_xrealloc2`: resize a block to `n` elements of `T`, with the multiply
/// checked by Ruby; a failure (`NoMemoryError`, `ArgumentError`) is `Err`, the
/// block untouched.
///
/// # Safety
/// `p` must be null or a live block from Ruby's allocator, and `n * size_of::<T>()`
/// must describe the new size.
#[inline]
pub unsafe fn realloc_array<T>(p: *mut T, n: usize) -> Result<*mut T, crate::bridge::rb::Error> {
    // SAFETY: the caller's contract; `ruby_xrealloc2` checks the multiply.
    unsafe { super::raw::xrealloc2(p as *mut c_void, n, core::mem::size_of::<T>()) }
        .map(|p| p as *mut T)
}

/// Why a [`RubyVec`] could not grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowError {
    /// The next capacity does not fit the element size.
    Overflow,
    /// Ruby's allocator raised (`NoMemoryError`); the array is unchanged.
    NoMemory,
}

/// A growable array of `Copy` elements held in Ruby's allocator.
///
/// Ruby's rather than Rust's on purpose: `ruby_xrealloc2` keeps the buffer
/// GC-accounted, so a large array contributes to the pressure that decides
/// when a GC runs, and it raises `NoMemoryError` instead of returning NULL,
/// and that raise comes back as [`GrowError::NoMemory`] (`raw`'s calls are
/// protected).
///
/// The one place the bridge spells out a Ruby-allocated buffer: its users
/// (`NodeSet`'s node array) are safe code over this.
pub struct RubyVec<T: Copy> {
    ptr: *mut T,
    len: usize,
    cap: usize,
}

/* SAFETY: the buffer is only ever read or written while the GVL is held, which
 * serialises every Ruby thread. The Send bound is for a wrapped value, which may
 * be freed on whichever thread runs the GC - still under the GVL. Nothing here is shared without it. The elements
 * must themselves permit transfer: Copy alone includes references to !Sync
 * data, which must not be made Send by placing them in this buffer. */
unsafe impl<T: Copy + Send> Send for RubyVec<T> {}

impl<T: Copy> Default for RubyVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy> RubyVec<T> {
    /// An empty array, holding no allocation.
    pub const fn new() -> Self {
        RubyVec {
            ptr: core::ptr::null_mut(),
            len: 0,
            cap: 0,
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
        // SAFETY: `ptr` is a live block of `cap >= len` elements, the first
        // `len` of them written by `push`.
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

    /// Reallocate to exactly `cap` elements.
    fn grow_to(&mut self, cap: usize) -> Result<(), GrowError> {
        // SAFETY: `ptr` is null or a live Ruby-allocated block of `self.cap`
        // elements; `realloc_array` reallocates it and checks the multiply.
        self.ptr = unsafe { realloc_array(self.ptr, cap) }.map_err(|_| GrowError::NoMemory)?;
        self.cap = cap;
        Ok(())
    }

    /// Append one element, growing geometrically (`falloc::grow_capacity`).
    pub fn push(&mut self, item: T) -> Result<(), GrowError> {
        if self.len == self.cap {
            let new_cap =
                crate::falloc::grow_capacity(self.cap, self.len + 1, core::mem::size_of::<T>())
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

impl<T: Copy> Drop for RubyVec<T> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: paired with `grow_to`'s allocation. Called from the GC's
            // free, where touching Ruby objects would be wrong but freeing our
            // own buffer is right.
            unsafe { free(self.ptr) };
        }
    }
}
