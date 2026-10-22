//! Kani proofs for the growable capped buffer.
//!
//! # What is proved, and against what
//!
//! A shadow copy of every byte ever appended, compared after each step: the
//! bytes survive every growth. Alongside it: the ceiling is respected, the
//! geometric-growth clamp holds (the room never passes the ceiling - the
//! property that stops a buffer allocating ~2x the hard maximum near the
//! limit), every failure leaves the buffer as it was, and `steal` hands back
//! exactly what went in.
//!
//! The buffer used to be libc memory grown by `realloc`, and these proofs were
//! written against that; when it was first made a `Vec` (2026-09) the shadow
//! proof ran CBMC out of memory, and the change was reverted
//! (`notes/cbuf_safe_rewrite_kani_failure.ja.md`). On Kani 0.67 the same
//! harness over the `Vec` takes ~26 s and ~1.8 GB, where the libc version took
//! ~174 s and ~4.7 GB - so the `Vec` was adopted, with the proofs kept at the
//! same bounds and the same properties, less the terminator there no longer is.
//!
//! Allocation failure is NOT reached here. The buffer consults
//! `falloc::allocation_should_fail`, which outside the `alloc-inject` build is
//! a constant false, and Kani runs CBMC with `--no-malloc-may-fail`, so no
//! allocation fails either way. The assertions on the OOM branches are
//! therefore vacuous under Kani; those branches are `rake oom`'s. The failure
//! these proofs do reach is the ceiling (`BufError::Limit`).
//!
//! Run with `rake kani`.

#![cfg(kani)]

use super::{Buf, BufError};

/// The largest ceiling the proofs quantify over.
///
/// Small on purpose, exactly as in the C harness these descend from: the LIMIT
/// branch is only reachable when the ceiling is within reach of a couple of
/// appends.
const MAXCAP: usize = 6;

/// The most bytes one append offers.
const NSRC: usize = 4;

/// Constrain the two build-time limits to the regime these proofs are about:
/// "for any hard maximum at least as large as the ceiling under test". Both are
/// Rust consts, so this is true rather than necessary; it stays because the
/// proof should say what it means, and because a constant that crossed a
/// language boundary would be an arbitrary value under Kani (the hazard that
/// made it load-bearing while the limits were C `extern static`s).
fn assume_limits_are_sane() {
    kani::assume(super::BUF_HARD_MAX >= MAXCAP);
    kani::assume(super::BUF_DEFAULT_LIMIT >= MAXCAP);
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
fn step_append(b: &mut Buf, shadow: &mut [u8], slen: usize, maxlim: usize) -> usize {
    let src: [u8; NSRC] = kani::any();
    let n: usize = kani::any();
    kani::assume(n <= NSRC);

    let len0 = b.len;
    let cap0 = b.data.len();
    let st = b.append(&src[..n]);

    if st.is_ok() {
        assert!(b.len == len0 + n, "append: len advances by n");
        assert!(b.len <= maxlim, "append: ceiling respected");
        assert!(
            b.data.len() <= maxlim,
            "append: growth clamped to the ceiling"
        );
        shadow[slen..slen + n].copy_from_slice(&src[..n]);
        let slen = slen + n;
        assert!(
            b.as_slice() == &shadow[..slen],
            "append: content matches the shadow"
        );
        slen
    } else {
        assert!(
            st != Err(BufError::Limit) || len0 + n > maxlim,
            "append: LIMIT only past the ceiling"
        );
        assert!(
            b.len == len0 && b.data.len() == cap0,
            "append: failure leaves the size intact"
        );
        assert!(
            b.as_slice() == &shadow[..slen],
            "append: failure leaves the content intact"
        );
        slen
    }
}

/// `copy_small` moves exactly its bytes for every length its fixed-size path
/// takes (0..=16) - the append proof offers at most `NSRC` = 4 bytes, so the
/// 4..=7 and 8..=16 pairs are proved here - and writes nothing else: the
/// destination is a window of a larger array, checked outside the window.
#[kani::proof]
#[kani::unwind(17)]
fn short_copies_move_exactly_their_bytes() {
    const W: usize = 16;
    let n: usize = kani::any();
    kani::assume(n <= W);
    let off: usize = kani::any();
    kani::assume(off <= W - n);
    let src: [u8; W] = kani::any();
    let mut dst = [0x5Au8; W];
    super::copy_small(&mut dst[off..off + n], &src[..n]);
    assert!(dst[off..off + n] == src[..n], "copy: the n bytes arrive");
    assert!(
        dst[..off].iter().chain(&dst[off + n..]).all(|&b| b == 0x5A),
        "copy: nothing outside the n bytes is written"
    );
}

/// Two appends with a reserve between them, against the shadow model: a small
/// nondet ceiling so LIMIT is reachable, and enough steps for growth, the clamp
/// and the ceiling failure to all occur (OOM is not reachable; see the module
/// doc).
#[kani::proof]
#[kani::unwind(40)]
fn append_matches_a_shadow_model() {
    assume_limits_are_sane();
    let max: usize = kani::any();
    kani::assume((1..=MAXCAP).contains(&max));
    let maxlim = limit_of(max);

    let mut b = Buf::new(max);
    let mut shadow = [0u8; 2 * NSRC];
    let mut slen = 0usize;

    slen = step_append(&mut b, &mut shadow, slen, maxlim);

    /* A reserve in the middle: it may grow the allocation but must never
     * change the content or the length. */
    let len0 = b.len;
    let cap0 = b.data.len();
    let want: usize = kani::any();
    kani::assume(want <= 2 * NSRC);
    let st = b.reserve(want);
    assert!(b.len == len0, "reserve: len untouched");
    assert!(
        b.data.len() <= maxlim,
        "reserve: clamped to the buffer's ceiling"
    );
    if st.is_ok() {
        assert!(b.data.len() >= cap0, "reserve: success does not shrink cap");
    } else {
        assert!(st == Err(BufError::Oom), "reserve: failure is OOM");
        assert!(b.data.len() == cap0, "reserve: failure leaves the capacity");
    }
    assert!(b.as_slice() == &shadow[..slen], "reserve: content intact");

    slen = step_append(&mut b, &mut shadow, slen, maxlim);

    /* Steal hands back exactly what went in, and leaves a usable empty
     * buffer. */
    let owned = b.steal();
    assert!(
        owned.as_slice() == &shadow[..slen],
        "steal: the bytes are what was appended"
    );
    assert!(
        b.len == 0 && b.data.is_empty(),
        "steal: buffer reset to empty"
    );
}

/// `steal` on a buffer nothing was ever appended to hands back an empty
/// result and leaves the buffer empty.
#[kani::proof]
#[kani::unwind(4)]
fn steal_of_an_empty_buffer_is_empty() {
    assume_limits_are_sane();
    let mut b = Buf::new(0);
    let owned = b.steal();
    assert!(
        owned.as_slice().is_empty(),
        "steal: an empty buffer has length 0"
    );
    assert!(b.len == 0 && b.data.is_empty(), "steal: still empty");
}
