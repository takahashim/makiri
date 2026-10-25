//! Ruby's allocator, for the few buffers Ruby's GC owns directly.
//!
//! Everything else in the extension allocates through `falloc`; a NodeSet's
//! node array is in Ruby's allocator so it is GC-accounted (`ruby_xrealloc2`
//! counts toward the pressure that decides when a GC runs). The array itself
//! is `crate::raw_vec::RawVec`, Ruby-free and proven there; this module is
//! the two calls it makes into Ruby.

#![allow(unsafe_code)]

use core::ffi::c_void;

pub use crate::raw_vec::GrowError;
use crate::raw_vec::{RawAlloc, RawVec};

/// Ruby's allocator as a [`RawAlloc`].
pub struct RubyAlloc;

// SAFETY: `ruby_xrealloc2` resizes a block of Ruby's allocator (or allocates
// one for null) keeping its contents, and checks the multiply; its raise
// (`NoMemoryError`, `ArgumentError`) comes back as `Err` - `raw` protects it -
// leaving the block untouched. `ruby_xfree` releases one.
unsafe impl RawAlloc for RubyAlloc {
    unsafe fn realloc<T>(p: *mut T, _old: usize, new: usize) -> Option<*mut T> {
        // SAFETY: the caller's contract.
        unsafe { super::raw::xrealloc2(p as *mut c_void, new, core::mem::size_of::<T>()) }
            .ok()
            .map(|p| p as *mut T)
    }

    unsafe fn free<T>(p: *mut T, _cap: usize) {
        // SAFETY: the caller's contract. Called from the GC's free, where
        // touching Ruby objects would be wrong but freeing a buffer is right.
        unsafe { rb_sys::ruby_xfree(p as *mut c_void) };
    }
}

/// A growable array of `Copy` elements held in Ruby's allocator. Its users
/// (`NodeSet`'s node array) are safe code over this.
pub type RubyVec<T> = RawVec<T, RubyAlloc>;
