//! The C allocator surface (core/mkr_alloc.c).
//!
//! Overflow-checked wrappers over libc `malloc`/`calloc`/`realloc` that fail
//! closed - every one returns NULL rather than aborting, wrapping or leaving the
//! caller's pointer in an implementation-defined state. C code still calls
//! these, and the memory is still libc's: `mkr_str_alloc`'s result is `free()`d
//! by its caller.
//!
//! # This module owns the injection counter
//!
//! `rake oom` works by failing the Nth allocation attempt of a run, and that
//! only means anything if there is exactly ONE counter. It used to live in C and
//! [`crate::falloc::should_fail`] consulted it across FFI; now it lives here and
//! the C consults it through `MKR_ALLOC_INJECT_FAIL()`, which resolves to
//! [`mkr_alloc_inject_should_fail`] at link time. The direction reversed; the
//! "exactly one" did not.
//!
//! The counter counts ATTEMPTS - every consult, armed or not - so the sweep can
//! size itself from a disarmed baseline run. Arming fails exactly one allocation
//! and then disarms, modelling a single transient OOM.
//!
//! Single-threaded by design, which holds because every caller is under the GVL
//! and the sweep is sequential. That was true of the C's plain `static`
//! variables too; the `static mut` here is the same object with the same
//! contract.

#![allow(clippy::missing_safety_doc)]

/* Everything below the injection counter serves the ported allocators only.
 * Gated rather than `allow(dead_code)`, so an unused item stays an error in the
 * configurations that should be using it. */
use core::ffi::{c_char, c_int, c_void};

use crate::cbuf::{MKR_ERR_OOM, MKR_OK};

extern "C" {
    #[link_name = "malloc"]
    fn libc_malloc(n: usize) -> *mut c_void;
    #[link_name = "calloc"]
    fn libc_calloc(count: usize, elem: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn libc_realloc(p: *mut c_void, n: usize) -> *mut c_void;
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
    #[link_name = "strlen"]
    fn libc_strlen(s: *const c_char) -> usize;
}

/* ------------------------------------------------------------------ *
 * failure injection                                                  *
 * ------------------------------------------------------------------ */

/* All three, not just the one the allocator itself calls: the Ruby test hooks
 * (`Makiri.__alloc_inject` / `__alloc_inject_calls`, bound in `init.rs`) arm and
 * read the same counter, and `inject` is private so that this re-export stays
 * the single way in. */
#[cfg(feature = "alloc-inject")]
pub use inject::{
    mkr_alloc_inject_arm, mkr_alloc_inject_calls, mkr_alloc_inject_should_fail,
};

#[cfg(feature = "alloc-inject")]
mod inject {
    /// 0 = disarmed.
    static mut COUNTDOWN: i64 = 0;
    static mut ATTEMPTS: u64 = 0;

    #[no_mangle]
    pub unsafe extern "C" fn mkr_alloc_inject_arm(nth: i64) {
        COUNTDOWN = if nth > 0 { nth } else { 0 };
        ATTEMPTS = 0;
    }

    #[no_mangle]
    pub unsafe extern "C" fn mkr_alloc_inject_calls() -> u64 {
        ATTEMPTS
    }

    #[no_mangle]
    pub unsafe extern "C" fn mkr_alloc_inject_should_fail() -> core::ffi::c_int {
        ATTEMPTS = ATTEMPTS.wrapping_add(1);
        if COUNTDOWN > 0 {
            COUNTDOWN -= 1;
            if COUNTDOWN == 0 {
                return 1; /* fail this one allocation; now disarmed */
            }
        }
        0
    }
}

/// Consult the injection counter. Always false outside a sweep build, where
/// there is no counter and no branch.
#[inline(always)]
fn inject_fail() -> bool {
    crate::falloc::should_fail()
}

/* ------------------------------------------------------------------ *
 * the allocators                                                     *
 * ------------------------------------------------------------------ */

/// `realloc` for `count * elem` bytes, overflow-checked.
///
/// `count == 0` FREES `ptr` and answers NULL - the one case that is not a
/// failure. `elem == 0` fails closed rather than falling through to a
/// `realloc(ptr, 0)`, whose free-or-not is implementation-defined; the caller
/// keeps ownership of `ptr`. An overflow leaves `ptr` unchanged.
#[no_mangle]
pub unsafe extern "C" fn mkr_reallocarray(
    ptr: *mut c_void,
    count: usize,
    elem: usize,
) -> *mut c_void {
    if count == 0 {
        libc_free(ptr);
        return core::ptr::null_mut();
    }
    if elem == 0 {
        return core::ptr::null_mut();
    }
    let bytes = match count.checked_mul(elem) {
        Some(b) => b,
        None => return core::ptr::null_mut(), /* overflow: ptr unchanged */
    };
    if inject_fail() {
        return core::ptr::null_mut();
    }
    libc_realloc(ptr, bytes)
}

/// Zeroed `count * elem` bytes.
///
/// Two-argument `calloc` is itself overflow-safe, but the check is explicit so
/// every core allocator fails the SAME way - a deterministic NULL - rather than
/// leaving the overflow case to `calloc`'s implementation-defined behaviour.
#[no_mangle]
pub unsafe extern "C" fn mkr_callocarray(count: usize, elem: usize) -> *mut c_void {
    if count == 0 || elem == 0 {
        return core::ptr::null_mut();
    }
    if count.checked_mul(elem).is_none() {
        return core::ptr::null_mut(); /* overflow */
    }
    if inject_fail() {
        return core::ptr::null_mut();
    }
    libc_calloc(count, elem)
}

/// `n` bytes plus a NUL terminator, with the terminator already written.
///
/// The bytes before it are uninitialised, as in the C: every caller fills them.
#[no_mangle]
pub unsafe extern "C" fn mkr_str_alloc(n: usize) -> *mut c_char {
    let total = match n.checked_add(1) {
        Some(t) => t,
        None => return core::ptr::null_mut(), /* n + 1 overflow */
    };
    if inject_fail() {
        return core::ptr::null_mut();
    }
    let p = libc_malloc(total) as *mut c_char;
    if p.is_null() {
        return core::ptr::null_mut();
    }
    *p.add(n) = 0;
    p
}

/// A NUL-terminated copy of `n` bytes from `s`.
///
/// `n > 0` with a NULL source fails closed: the alternative is returning
/// uninitialised bytes.
#[no_mangle]
pub unsafe extern "C" fn mkr_strndup(s: *const c_char, n: usize) -> *mut c_char {
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

/// A NUL-terminated copy of the C string `s`. NULL in, NULL out.
#[no_mangle]
pub unsafe extern "C" fn mkr_strdup(s: *const c_char) -> *mut c_char {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    mkr_strndup(s, libc_strlen(s))
}

/// Grow `*ptr` to hold at least `need` elements of `elem` bytes, geometrically.
///
/// On success `*ptr` and `*cap` are updated and `MKR_OK` is returned; on
/// overflow or allocation failure `MKR_ERR_OOM` is returned with both left
/// unchanged - so a failed grow never loses the caller's array.
#[no_mangle]
pub unsafe extern "C" fn mkr_grow_reserve(
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
    let p = mkr_reallocarray(*ptr, new_cap, elem);
    if p.is_null() {
        return MKR_ERR_OOM;
    }
    *ptr = p;
    *cap = new_cap;
    MKR_OK
}
