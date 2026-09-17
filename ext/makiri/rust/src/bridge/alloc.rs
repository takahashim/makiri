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
/// checked by Ruby (`RangeError`/`NoMemoryError` on failure).
///
/// # Safety
/// `p` must be null or a live block from Ruby's allocator, and `n * size_of::<T>()`
/// must describe the new size.
#[inline]
pub unsafe fn realloc_array<T>(p: *mut T, n: usize) -> *mut T {
    // SAFETY: the caller's contract; `ruby_xrealloc2` checks the multiply.
    unsafe {
        rb_sys::ruby_xrealloc2(
            p as *mut c_void,
            n as rb_sys::size_t,
            core::mem::size_of::<T>() as rb_sys::size_t,
        ) as *mut T
    }
}
