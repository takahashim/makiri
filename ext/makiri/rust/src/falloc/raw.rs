//! The raw allocator primitives the Kani ownership proofs quantify over.
//!
//! Nothing on a production path calls these any more: the typed API in the
//! parent module and `cstr` cover every live allocation. They stay because the
//! proofs must talk about a concrete libc operation, and `calloc_verify` is
//! where the boundary contract (NULL means the caller still owns the old block;
//! a rejected size never frees) is checked.
//!
//! `falloc/mod.rs` carries `#![allow(unsafe_code)]` for the whole subtree, so
//! this file does not restate it.

use core::ffi::c_void;

extern "C" {
    #[link_name = "calloc"]
    fn libc_calloc(count: usize, elem: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn libc_realloc(p: *mut c_void, n: usize) -> *mut c_void;
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

#[inline(always)]
fn allocation_should_fail() -> bool {
    crate::falloc::allocation_should_fail()
}

/// Reallocate `ptr` for `count * elem` bytes.
///
/// A zero count or element size is rejected without touching `ptr`. Releasing an
/// existing allocation is the separate `free_and_null`, so a NULL result from
/// this function always means the caller still owns the old block.
pub(crate) unsafe fn reallocarray(ptr: *mut c_void, count: usize, elem: usize) -> *mut c_void {
    if count == 0 || elem == 0 {
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
pub(crate) unsafe fn free_and_null(ptr: *mut c_void) -> *mut c_void {
    libc_free(ptr);
    core::ptr::null_mut()
}

/// A zeroed `count * elem`-byte allocation.
pub(crate) unsafe fn callocarray(count: usize, elem: usize) -> *mut c_void {
    if count == 0 || elem == 0 || count.checked_mul(elem).is_none() {
        return core::ptr::null_mut();
    }
    if allocation_should_fail() {
        return core::ptr::null_mut();
    }
    libc_calloc(count, elem)
}
