//! Lexbor's allocator, with slack: every block Lexbor takes from the heap is
//! [`SLACK`] bytes longer than it asked for, so a write a few bytes past the
//! end of one lands in bytes nothing else owns and nothing reads.
//!
//! Hardening for the one component written in C: an off-by-a-few overrun in
//! vendored Lexbor becomes harmless instead of corrupting the neighbouring
//! allocation. Lexbor is never patched here, so this uses the hook Lexbor
//! itself offers (`lexbor_memory_setup`).
//!
//! What it does not do: an overrun longer than [`SLACK`] still corrupts, and
//! one INSIDE an arena chunk (`mraw`, from one sub-allocation into the next)
//! is not past any heap block at all. Under the sanitizer (`makiri_asan`) it
//! is not installed, because there the point is to see such a write.
//!
//! The four functions are called from C and must not unwind into it: their
//! bodies are an overflow-checked addition and a C call, with nothing that can
//! panic, which is why they carry no panic latch (the same reasoning as the
//! GC callbacks in `bridge/typed.rs`).

#![allow(unsafe_code)]
// Under the sanitizer nothing is installed, so the functions go unused.
#![cfg_attr(makiri_asan, allow(dead_code))]

use core::ffi::c_void;

use crate::lexbor::abi as lxb;

/// The bytes added past every Lexbor heap block - a multiple of the
/// allocator's alignment, so the blocks' own alignment is unchanged.
pub const SLACK: usize = 16;

unsafe extern "C" fn slack_malloc(size: usize) -> *mut c_void {
    match size.checked_add(SLACK) {
        // SAFETY: the C allocator, called as C calls it.
        Some(n) => unsafe { lxb::malloc(n) },
        None => core::ptr::null_mut(),
    }
}

unsafe extern "C" fn slack_realloc(dst: *mut c_void, size: usize) -> *mut c_void {
    match size.checked_add(SLACK) {
        // SAFETY: `dst` is null or a block of this allocator, as Lexbor
        // passes it.
        Some(n) => unsafe { lxb::realloc(dst, n) },
        None => core::ptr::null_mut(),
    }
}

unsafe extern "C" fn slack_calloc(num: usize, size: usize) -> *mut c_void {
    match num.checked_mul(size).and_then(|n| n.checked_add(SLACK)) {
        // SAFETY: the C allocator, called as C calls it.
        Some(n) => unsafe { lxb::calloc(1, n) },
        None => core::ptr::null_mut(),
    }
}

unsafe extern "C" fn slack_free(dst: *mut c_void) {
    // SAFETY: `dst` is null or a block of this allocator, as Lexbor passes it.
    unsafe { lxb::free(dst) }
}

/// Point Lexbor's allocator at the functions above - at `Init_makiri`,
/// before the first parse. A block Lexbor took before this is still the C
/// allocator's, so freeing or growing it here is still right. Nothing to do
/// under the sanitizer (module doc).
pub fn install() {
    #[cfg(not(makiri_asan))]
    // SAFETY: four non-null functions with Lexbor's signatures; the call only
    // stores them. It fails only for a null one, which none is.
    unsafe {
        lxb::lexbor_memory_setup(
            Some(slack_malloc),
            Some(slack_realloc),
            Some(slack_calloc),
            Some(slack_free),
        );
    }
}
