//! Kani proofs for the growable capped buffer.
//!
//! The translation of `verify/harness_buf.c`, which checked `core/mkr_buf.c`
//! against a shadow reference model.
//!
//! While both existed they proved DIFFERENT builds - the C harness covered the
//! build that linked `core/mkr_buf.c`, these cover the build that links this
//! module, and neither statement was the other. That distinction is why both
//! were kept. Only one build remains, so only these do.
//!
//! # What is proved, and against what
//!
//! A shadow copy of every byte ever appended, compared after each step. That is
//! the property no type gives for free: the buffer reallocs through libc, and
//! "the bytes survived the move" is a claim about the allocator's behaviour, not
//! about Rust's. Kani models `realloc` well enough to carry it - checked before
//! this file was written rather than assumed, because if it could not, porting
//! `mkr_buf.c` would have traded a real proof for a weaker one.
//!
//! Alongside it: the ceiling is respected, the geometric-growth clamp holds
//! (`cap <= limit + 1`, the property that stops a buffer allocating ~2x the hard
//! maximum near the limit), every failure leaves the buffer bit-for-bit as it
//! was, and `steal` hands back exactly what went in.
//!
//! Allocation failure is reachable here the same way `rake oom` reaches it: the
//! implementation consults `falloc::should_fail`, so a Kani-nondeterministic
//! injection counter drives every OOM branch.
//!
//! Run with `rake kani`.

#![cfg(kani)]

use super::{
    buf_append, buf_reserve, buf_steal, Buf, BUF_ERR_INVALID, BUF_ERR_LIMIT, BUF_ERR_OOM, BUF_OK,
};

/// The largest ceiling the proofs quantify over.
///
/// Small on purpose, exactly as in the C harness: the LIMIT branch is only
/// reachable when the ceiling is within reach of a couple of appends. Six is
/// what the C used.
const MAXCAP: usize = 6;

/// The most bytes one append offers.
const NSRC: usize = 4;

/// Constrain the two build-time limits to the regime these proofs are about.
///
/// **This was load-bearing, and the reason it existed is worth keeping.** While
/// the C was compiled, `buf_hard_max` and `buf_default_limit` were
/// `extern static`s defined in `core/mkr_core_abi.c` - and Kani does not link C,
/// so without this assumption they were UNCONSTRAINED values. The first version
/// of this file omitted it and the proof duly failed: `content_limit` took
/// `min(max, <anything>)`, so LIMIT could fire below the ceiling the harness
/// thought it had set, and "LIMIT only past the ceiling" was false.
///
/// That was a general hazard, not a local slip: any Rust reading a C-defined
/// constant is, under Kani, reading an arbitrary value. Both are ordinary Rust
/// consts now, so the assumption is trivially true rather than necessary. It
/// stays because the
/// proof should keep saying what it means ("for any hard maximum at least as
/// large as the ceiling under test") and because the hazard returns the moment
/// any constant crosses a language boundary again.
unsafe fn assume_limits_are_sane() {
    kani::assume(super::buf_hard_max >= MAXCAP);
    kani::assume(super::buf_default_limit >= MAXCAP);
}

/// The effective ceiling for a buffer whose `max` is in the assumed range.
///
/// Recomputed here rather than read from the implementation: a reference model
/// that agreed by calling the thing it checks would prove nothing. With `max`
/// small and the hard maximum constrained above, `min(max, hard)` is `max`.
fn limit_of(max: usize) -> usize {
    max
}

/// One nondet append, checked against the shadow model.
///
/// Returns the new shadow length.
unsafe fn step_append(b: &mut Buf, shadow: &mut [u8], slen: usize, maxlim: usize) -> usize {
    let src: [u8; NSRC] = kani::any();
    let n: usize = kani::any();
    kani::assume(n <= NSRC);

    let len0 = b.len;
    let cap0 = b.cap;
    let st = buf_append(b, src.as_ptr() as *const core::ffi::c_void, n);

    if st == BUF_OK {
        assert!(b.len == len0 + n, "append: len advances by n");
        assert!(b.len <= maxlim, "append: ceiling respected");
        assert!(b.cap <= maxlim + 1, "append: growth clamped to max+1");
        if n != 0 {
            assert!(*b.data.add(b.len) == 0, "append: NUL-terminated");
        }
        shadow[slen..slen + n].copy_from_slice(&src[..n]);
        let slen = slen + n;
        assert!(
            content_matches(b, &shadow[..slen]),
            "append: content matches the shadow"
        );
        slen
    } else {
        assert!(
            st == BUF_ERR_LIMIT || st == BUF_ERR_OOM,
            "append: a non-NULL source fails only on limit or OOM"
        );
        assert!(
            st != BUF_ERR_LIMIT || len0 + n > maxlim,
            "append: LIMIT only past the ceiling"
        );
        assert!(
            b.len == len0 && b.cap == cap0,
            "append: failure leaves the size intact"
        );
        assert!(
            content_matches(b, &shadow[..slen]),
            "append: failure leaves the content intact"
        );
        slen
    }
}

/// The buffer's bytes equal `want`.
unsafe fn content_matches(b: &Buf, want: &[u8]) -> bool {
    if want.is_empty() {
        return true;
    }
    if b.data.is_null() {
        return false;
    }
    core::slice::from_raw_parts(b.data as *const u8, want.len()) == want
}

/// Two appends with a reserve between them, against the shadow model.
///
/// The C harness's `main`, with the same shape and the same bounds: a small
/// nondet ceiling so LIMIT is reachable, and enough steps for growth, the clamp
/// and the failure paths to all occur.
#[kani::proof]
#[kani::unwind(40)]
fn append_matches_a_shadow_model() {
    unsafe { assume_limits_are_sane() };
    let max: usize = kani::any();
    kani::assume((1..=MAXCAP).contains(&max));
    let maxlim = limit_of(max);

    let mut b = Buf::new(max);
    let mut shadow = [0u8; 2 * NSRC];
    let mut slen = 0usize;

    unsafe {
        /* A NULL source with n > 0 is INVALID and touches nothing. */
        assert!(
            buf_append(&mut b, core::ptr::null(), 3) == BUF_ERR_INVALID,
            "append: a NULL source fails closed"
        );
        assert!(b.len == 0 && b.cap == 0, "append: INVALID touches nothing");

        slen = step_append(&mut b, &mut shadow, slen, maxlim);

        /* A reserve in the middle: it may grow the allocation but must never
         * change the content or the length. */
        let len0 = b.len;
        let cap0 = b.cap;
        let want: usize = kani::any();
        kani::assume(want <= 2 * NSRC);
        let st = buf_reserve(&mut b, want);
        assert!(b.len == len0, "reserve: len untouched");
        assert!(
            b.cap <= maxlim + 1,
            "reserve: clamped to the buffer's ceiling"
        );
        if st == BUF_OK {
            assert!(b.cap >= cap0, "reserve: success does not shrink cap");
        } else {
            assert!(st == BUF_ERR_OOM, "reserve: failure is OOM");
            assert!(
                b.len == len0 && b.cap == cap0,
                "reserve: failure leaves len and cap"
            );
        }
        assert!(
            content_matches(&b, &shadow[..slen]),
            "reserve: content intact"
        );

        slen = step_append(&mut b, &mut shadow, slen, maxlim);

        /* Steal hands back exactly what went in, NUL-terminated, and leaves a
         * usable empty buffer. */
        let mut out_len = usize::MAX;
        let p = buf_steal(&mut b, &mut out_len);
        if !p.is_null() {
            assert!(out_len == slen, "steal: the length is what was appended");
            let got = core::slice::from_raw_parts(p as *const u8, out_len);
            assert!(
                got == &shadow[..slen],
                "steal: the bytes are what was appended"
            );
            assert!(*p.add(out_len) == 0, "steal: NUL-terminated");
            libc_free(p as *mut core::ffi::c_void);
        }
        assert!(
            b.data.is_null() && b.len == 0 && b.cap == 0,
            "steal: buffer reset to empty"
        );
    }
}

/// `steal` on a buffer nothing was ever appended to.
///
/// It must hand back a freshly owned `""` rather than NULL, so a caller never
/// has to tell "no output" from "failed" - which makes NULL unambiguously OOM.
#[kani::proof]
#[kani::unwind(4)]
fn steal_of_an_empty_buffer_is_an_owned_empty_string() {
    unsafe { assume_limits_are_sane() };
    let mut b = Buf::new(0);
    unsafe {
        let mut out_len = usize::MAX;
        let p = buf_steal(&mut b, &mut out_len);
        if p.is_null() {
            /* The only reason is allocation failure. */
        } else {
            assert!(out_len == 0, "steal: an empty buffer has length 0");
            assert!(*p == 0, "steal: the empty result is NUL-terminated");
            libc_free(p as *mut core::ffi::c_void);
        }
        assert!(
            b.data.is_null() && b.len == 0 && b.cap == 0,
            "steal: still empty"
        );
    }
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut core::ffi::c_void);
}
