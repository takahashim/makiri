//! Kani proofs for the growable capped buffer.
//!
//! What is left here is what is cheap to prove for every input: that
//! `copy_small` moves exactly its bytes, and that stealing from an empty buffer
//! leaves it empty.
//!
//! The shadow-model proof - two appends with a reserve between them, the
//! bytes compared after every step, the ceiling and the growth clamp checked,
//! failures shown to change nothing - stood here too. Over the `Vec` it ran
//! CBMC out of memory (3-4 GB and 10+ minutes for one harness on Kani 0.67,
//! the CI runner's four parallel jobs cancelled), so it is now an exhaustive
//! test over the same small domain
//! (`cbuf::tests::append_matches_a_shadow_model_over_the_whole_small_domain`):
//! every ceiling up to 6, append up to 4 bytes and reserve up to 8, 1,350 cases,
//! with distinct bytes per position in place of symbolic ones - the code never
//! branches on a byte's value.
//!
//! Run with `rake kani`.

#![cfg(kani)]

use super::Buf;

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
    kani::cover!(n == W, "a full 16-byte copy");
    kani::cover!(
        n > 0 && off > 0 && off + n < W,
        "a copy with bytes on both sides"
    );
}

/// `steal` on a buffer nothing was ever appended to hands back an empty
/// result and leaves the buffer empty.
#[kani::proof]
#[kani::unwind(4)]
fn steal_of_an_empty_buffer_is_empty() {
    let mut b = Buf::new(0);
    let owned = b.steal();
    assert!(
        owned.as_slice().is_empty(),
        "steal: an empty buffer has length 0"
    );
    assert!(b.len == 0 && b.data.is_empty(), "steal: still empty");
    kani::cover!(true, "the steal returns");
}
