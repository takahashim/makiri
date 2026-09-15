//! Raw libc allocation used at C ABI boundaries.
//!
//! These functions intentionally return raw pointers: their allocations are
//! released by C-compatible destructors. Rust-owned data should use the
//! fallible typed APIs in `falloc::mod` instead.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use crate::cbuf::{MKR_ERR_OOM, MKR_OK};

extern "C" {
    #[link_name = "calloc"]
    fn libc_calloc(count: usize, elem: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn libc_realloc(p: *mut c_void, n: usize) -> *mut c_void;
    #[cfg(kani)]
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

#[inline(always)]
fn allocation_should_fail() -> bool {
    crate::falloc::allocation_should_fail()
}

/// Reallocate `ptr` for `count * elem` bytes.
///
/// A zero count is rejected without touching `ptr`. Releasing an existing
/// allocation is a separate operation, `free_and_null`, so a NULL result
/// from this function always means that the caller still owns the old block.
pub(crate) unsafe fn reallocarray(ptr: *mut c_void, count: usize, elem: usize) -> *mut c_void {
    if count == 0 {
        return core::ptr::null_mut();
    }
    if elem == 0 {
        return core::ptr::null_mut();
    }
    let bytes = match count.checked_mul(elem) {
        Some(b) => b,
        None => return core::ptr::null_mut(),
    };
    if allocation_should_fail() {
        return core::ptr::null_mut();
    }
    libc_realloc(ptr, bytes)
}

/// Release a libc allocation and return a null pointer for slot replacement.
/// Only the allocator proofs still release this way.
#[cfg(kani)]
pub(crate) unsafe fn free_and_null(ptr: *mut c_void) -> *mut c_void {
    libc_free(ptr);
    core::ptr::null_mut()
}

pub(crate) unsafe fn callocarray(count: usize, elem: usize) -> *mut c_void {
    if count == 0 || elem == 0 || count.checked_mul(elem).is_none() {
        return core::ptr::null_mut();
    }
    if allocation_should_fail() {
        return core::ptr::null_mut();
    }
    libc_calloc(count, elem)
}

pub unsafe fn grow_reserve(
    ptr: *mut *mut c_void,
    cap: *mut usize,
    need: usize,
    elem: usize,
) -> c_int {
    if need <= *cap {
        return MKR_OK;
    }
    let new_cap = match crate::falloc::grow_capacity(*cap, need, elem) {
        Some(c) => c,
        None => return MKR_ERR_OOM,
    };
    let p = reallocarray(*ptr, new_cap, elem);
    if p.is_null() {
        return MKR_ERR_OOM;
    }
    *ptr = p;
    *cap = new_cap;
    MKR_OK
}
