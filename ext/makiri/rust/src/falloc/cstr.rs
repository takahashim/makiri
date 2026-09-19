//! Fallible NUL-terminated C-string allocations.
//!
//! `falloc/mod.rs` carries `#![allow(unsafe_code)]` for the whole subtree, so
//! this file does not restate it.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_void};

extern "C" {
    #[link_name = "malloc"]
    fn libc_malloc(n: usize) -> *mut c_void;
}

#[inline(always)]
fn allocation_should_fail() -> bool {
    crate::falloc::allocation_should_fail()
}

/// `n + 1` bytes from libc, NUL-terminated at `n`, or NULL on overflow, OOM, or
/// an armed failure hook.
pub unsafe fn str_alloc(n: usize) -> *mut c_char {
    let total = match n.checked_add(1) {
        Some(t) => t,
        None => return core::ptr::null_mut(),
    };
    if allocation_should_fail() {
        return core::ptr::null_mut();
    }
    let p = libc_malloc(total) as *mut c_char;
    if p.is_null() {
        return core::ptr::null_mut();
    }
    *p.add(n) = 0;
    p
}

/// Copy exactly `n` bytes of `s`, NUL-terminated. Only the allocator proof
/// calls this; no production path does.
#[cfg(kani)]
pub unsafe fn strndup(s: *const c_char, n: usize) -> *mut c_char {
    if n > 0 && s.is_null() {
        return core::ptr::null_mut();
    }
    let p = str_alloc(n);
    if p.is_null() {
        return core::ptr::null_mut();
    }
    if n > 0 {
        core::ptr::copy_nonoverlapping(s, p, n);
    }
    p
}
