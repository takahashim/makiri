//! Fallible NUL-terminated C-string allocations.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_void};

extern "C" {
    #[link_name = "malloc"]
    fn libc_malloc(n: usize) -> *mut c_void;
    #[link_name = "strlen"]
    fn libc_strlen(s: *const c_char) -> usize;
}

#[inline(always)]
fn allocation_should_fail() -> bool {
    crate::falloc::allocation_should_fail()
}

pub unsafe fn mkr_str_alloc(n: usize) -> *mut c_char {
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

pub unsafe fn mkr_strndup(s: *const c_char, n: usize) -> *mut c_char {
    if n > 0 && s.is_null() {
        return core::ptr::null_mut();
    }
    let p = mkr_str_alloc(n);
    if p.is_null() {
        return core::ptr::null_mut();
    }
    if n > 0 {
        core::ptr::copy_nonoverlapping(s, p, n);
    }
    *p.add(n) = 0;
    p
}

pub unsafe fn mkr_strdup(s: *const c_char) -> *mut c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    mkr_strndup(s, libc_strlen(s))
}
