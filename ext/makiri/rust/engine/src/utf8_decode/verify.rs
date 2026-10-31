//! Kani proofs for the shared UTF-8 primitives.
//!
//! These are what `verify/harness_utf8.c` and `harness_utf8_chain.c` became for
//! the Rust build. `harness_utf8_words.c` has no counterpart on purpose: its
//! subject was the C's word-at-a-time ASCII scan, which this module does not
//! have - `core::str::from_utf8` brings its own, and it is not ours to prove.
//!
//! The CBMC harnesses covered `core/mkr_utf8.c`, and these cover the Rust that
//! replaced it. Neither statement was the other, which is why both existed
//! while both implementations did. `core/mkr_utf8.c` is gone, and so are they.
//!
//! One harness is left. `decode1`'s own contract (length, scalar range, no
//! surrogate) and the whole-buffer chain (the decodes consume valid input
//! exactly), which were proved here up to 8 bytes, are `verus/utf8.rs`'s now,
//! at every length - against vstd's statement of UTF-8. What stays is the tie
//! between that statement and std's validator, `from_utf8`, which Verus has no
//! contract for: exhaustive over the decoder's 4-byte input.
//!
//! Run with `rake kani` (or `cargo kani --no-default-features`).

#![forbid(unsafe_code)]
#![cfg(kani)]

use super::{decode1, valid};

/// The load-bearing one: our decoder and the standard library agree.
///
/// This is the cross-check the C proved between its own two implementations.
/// Here one side is `core::str::from_utf8`, which we did not write - so the
/// property is, if anything, stronger evidence than it was.
///
/// Both directions, because "neither side is stricter than the other" is the
/// claim: a prefix the decoder accepts must be valid UTF-8, and the first code
/// point of a valid buffer must decode.
#[kani::proof]
#[kani::unwind(6)]
fn decode1_agrees_with_from_utf8() {
    let buf: [u8; 4] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= buf.len());
    let s = &buf[..len];

    match decode1(s) {
        Some((cp, n)) => {
            assert!(
                valid(&s[..n]),
                "decode1: the accepted prefix is valid UTF-8"
            );
            /* And it decodes to the same code point the standard library sees. */
            let first = core::str::from_utf8(&s[..n])
                .unwrap()
                .chars()
                .next()
                .unwrap();
            assert!(first as u32 == cp, "decode1: same code point as from_utf8");
            kani::cover!(n == 4, "a 4-byte code point is compared");
        }
        None => {
            /* Nothing to assert about an arbitrary rejection: a valid buffer's
             * FIRST code point always decodes, which is the direction below. */
        }
    }

    if valid(s) && len > 0 {
        assert!(decode1(s).is_some(), "valid non-empty input decodes");
        kani::cover!(len == 4, "a valid 4-byte buffer");
    }
}
