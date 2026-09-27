//! Fallible NUL-terminated C-string allocations.
//!
//! `falloc/mod.rs` carries `#![allow(unsafe_code)]` for the whole subtree, so
//! this file does not restate it.

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
///
/// Safe: it has no precondition on its argument and returns a raw pointer the
/// caller owns. The `unsafe` is the terminator write into the fresh block.
pub fn str_alloc(n: usize) -> *mut c_char {
    let total = match n.checked_add(1) {
        Some(t) => t,
        None => return core::ptr::null_mut(),
    };
    if allocation_should_fail() {
        return core::ptr::null_mut();
    }
    // SAFETY: `malloc(total)` returns null or `total == n + 1` writable bytes,
    // so byte `n` is in bounds; the allocation is not shared yet. A null result
    // is handed back to the caller as "failed".
    unsafe {
        let p = libc_malloc(total) as *mut c_char;
        if p.is_null() {
            return core::ptr::null_mut();
        }
        *p.add(n) = 0;
        p
    }
}

mod verify;
