//! Kani proofs for the C allocator surface.
//!
//! `verify/harness_alloc.c` proved three things about `core/mkr_alloc.c`, and
//! they went to three different places:
//!
//! - **the size arithmetic** (`mkr_size_add`, `mkr_size_mul`): gone as an
//!   obligation. `checked_add` / `checked_mul` are the standard library's.
//! - **`mkr_grow_capacity`**: already proved in `falloc::verify`, where Kani
//!   found a real precondition the C harness had not.
//! - **the OOM branches**: `rake oom`, end to end. Not a proof, but it exercises
//!   the whole extension rather than one function, and the sweep's injection
//!   points are identical to the C build's.
//!
//! What is left is the OWNERSHIP contract at the boundary, which none of those
//! three covers and which is the part that turns into a double free or a leak
//! when it is wrong. That is what this file proves.
//!
//! Run with `rake kani`.

#![cfg(kani)]
#![cfg(feature = "core-alloc")]

use core::ffi::c_void;

use super::calloc::{mkr_callocarray, mkr_reallocarray, mkr_str_alloc, mkr_strndup};

/// `mkr_reallocarray`'s three non-allocating answers differ in who owns `ptr`
/// afterwards, and getting that wrong is a double free or a leak.
///
/// - `count == 0` FREES `ptr` and answers NULL. The one case that is not a
///   failure, and the one where ownership transfers.
/// - `elem == 0` answers NULL and does NOT free: the caller keeps `ptr`. The C
///   comment is explicit that this exists so the call never falls through to a
///   `realloc(ptr, 0)`, whose free-or-not is implementation-defined.
/// - an overflowing `count * elem` answers NULL and leaves `ptr` untouched.
///
/// The proof is that the last two leave the allocation usable: Kani's memory
/// model reports a use-after-free, so writing through `ptr` afterwards is what
/// makes "did not free" a checked claim rather than a comment.
#[kani::proof]
#[kani::unwind(4)]
fn reallocarray_ownership() {
    unsafe {
        /* elem == 0: NULL, and the caller still owns ptr. */
        let p = mkr_callocarray(4, 1);
        if !p.is_null() {
            let count: usize = kani::any();
            kani::assume(count > 0);
            let r = mkr_reallocarray(p, count, 0);
            assert!(r.is_null(), "elem == 0 answers NULL");
            *(p as *mut u8) = 7; /* still ours: a freed one would be caught here */
            assert!(*(p as *const u8) == 7);
            free(p);
        }

        /* An overflowing size: NULL, and the caller still owns ptr. */
        let q = mkr_callocarray(4, 1);
        if !q.is_null() {
            let r = mkr_reallocarray(q, usize::MAX, 2);
            assert!(r.is_null(), "an overflowing size answers NULL");
            *(q as *mut u8) = 9;
            assert!(*(q as *const u8) == 9);
            free(q);
        }

        /* count == 0 frees. Nothing is asserted about `ptr` afterwards for the
         * obvious reason - it is gone, and reading it is the bug this case
         * exists to let callers avoid. */
        let z = mkr_callocarray(4, 1);
        if !z.is_null() {
            assert!(mkr_reallocarray(z, 0, 1).is_null(), "count == 0 answers NULL");
        }
    }
}

/// `mkr_callocarray` answers NULL for a zero dimension without allocating, and
/// zeroes what it does allocate.
#[kani::proof]
#[kani::unwind(8)]
fn callocarray_zeroes_and_rejects_zero_dimensions() {
    unsafe {
        let n: usize = kani::any();
        kani::assume(n <= 4);
        assert!(mkr_callocarray(n, 0).is_null(), "elem == 0 allocates nothing");
        assert!(mkr_callocarray(0, n).is_null(), "count == 0 allocates nothing");

        kani::assume(n > 0);
        let p = mkr_callocarray(n, 1) as *mut u8;
        if !p.is_null() {
            for i in 0..n {
                assert!(*p.add(i) == 0, "callocarray: the bytes are zeroed");
            }
            free(p as *mut c_void);
        }
    }
}

/// `mkr_str_alloc` writes the terminator, and `mkr_strndup` copies exactly `n`
/// bytes and terminates after them.
///
/// The terminator is the whole point of these two over a bare `malloc`: every
/// caller hands the result to something that reads it as a C string.
#[kani::proof]
#[kani::unwind(8)]
fn str_alloc_and_strndup_terminate() {
    unsafe {
        let n: usize = kani::any();
        kani::assume(n <= 4);

        let p = mkr_str_alloc(n);
        if !p.is_null() {
            assert!(*p.add(n) == 0, "str_alloc: terminated at n");
            free(p as *mut c_void);
        }

        let src: [u8; 4] = kani::any();
        let d = mkr_strndup(src.as_ptr() as *const core::ffi::c_char, n);
        if !d.is_null() {
            for i in 0..n {
                assert!(*d.add(i) as u8 == src[i], "strndup: copies the bytes");
            }
            assert!(*d.add(n) == 0, "strndup: terminated at n");
            free(d as *mut c_void);
        }

        /* A NULL source with n > 0 fails closed rather than returning
         * uninitialised bytes. */
        kani::assume(n > 0);
        assert!(
            mkr_strndup(core::ptr::null(), n).is_null(),
            "strndup: a NULL source with n > 0 fails closed"
        );
    }
}

extern "C" {
    #[link_name = "free"]
    fn free(p: *mut c_void);
}
