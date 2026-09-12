//! Kani proofs for the shared UTF-8 primitives.
//!
//! These are what `verify/harness_utf8.c` and `harness_utf8_chain.c` became for
//! the Rust build. `harness_utf8_words.c` has no counterpart on purpose: its
//! subject was the C's word-at-a-time ASCII scan, which this module does not
//! have - `core::str::from_utf8` brings its own, and it is not ours to prove.
//!
//! The CBMC harnesses still run under `rake verify` and still pass. They cover
//! `core/mkr_utf8.c`, which is what a build without `MAKIRI_RUST_CORE_UTF8`
//! links; these cover what a build with it links. Neither statement is the
//! other, which is why both exist.
//!
//! Run with `rake kani` (or `cargo kani --features xml,xpath`).

#![cfg(kani)]

use crate::kani_bounds::parse_usize;

use super::{decode1, valid};

/// The longest input the whole-buffer proof quantifies over.
///
/// The retired C harness used 12, chosen there to cover a full word-at-a-time
/// iteration plus the word-to-byte-tail transition. There is no word scan here,
/// so the bound is about the CHAIN: 8 bytes is two full 4-byte code points plus
/// slack, which is what the chaining property needs to bite.
///
/// Raise it with `KANI_UTF8_CHAIN_MAX` when a deeper run is worth the wait.
const N: usize = match option_env!("KANI_UTF8_CHAIN_MAX") {
    Some(s) => parse_usize(s),
    None => 8,
};

/// The decoder's own contract, exhaustive over its input unit.
///
/// A code point is at most 4 bytes, so a nondet 4-byte buffer covers every
/// input the decoder can be asked about - this is a complete check, not a
/// sample. The same shape as the C harness's first block.
///
/// The unwind bound is not optional. `decode1`'s continuation loop runs over a
/// slice whose length Kani cannot infer from `p.get(1..len)`, so without a bound
/// it unwinds forever - it was still going at iteration 920 after four minutes,
/// which is how this was found. A code point's tail is at most three bytes, so
/// four is a bound with slack.
#[kani::proof]
#[kani::unwind(4)]
fn decode1_is_strict() {
    let buf: [u8; 4] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= buf.len());

    if let Some((cp, n)) = decode1(&buf[..len]) {
        assert!((1..=4).contains(&n), "decode1: length is 1..=4");
        assert!(n <= len, "decode1: length within the input");
        assert!(cp <= 0x10FFFF, "decode1: scalar range");
        assert!(!(0xD800..=0xDFFF).contains(&cp), "decode1: no surrogates");
    }
}

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
            assert!(valid(&s[..n]), "decode1: the accepted prefix is valid UTF-8");
            /* And it decodes to the same code point the standard library sees. */
            let first = core::str::from_utf8(&s[..n]).unwrap().chars().next().unwrap();
            assert!(first as u32 == cp, "decode1: same code point as from_utf8");
        }
        None => {
            /* Nothing to assert about an arbitrary rejection: a valid buffer's
             * FIRST code point always decodes, which is the direction below. */
        }
    }

    if valid(s) && len > 0 {
        assert!(decode1(s).is_some(), "valid non-empty input decodes");
    }
}

/// Whole-buffer agreement: chaining the decoder over a valid buffer consumes it
/// exactly, and a buffer the chain consumes exactly is valid.
///
/// The C's `harness_utf8_chain.c`, which closed the loop that the first-code-
/// point proof leaves open - neither side may be stricter than the other over a
/// sequence, not just over one code point.
#[kani::proof]
#[kani::unwind(10)]
fn chain_consumes_exactly_valid_input() {
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= buf.len());
    let s = &buf[..len];

    let ok = valid(s);

    /* Each accepted code point is at least one byte, so at most `len` steps. */
    let mut off = 0usize;
    let mut consumed_all = true;
    for _ in 0..N {
        if off >= len {
            break;
        }
        match decode1(&s[off..]) {
            Some((_, n)) => off += n,
            None => {
                consumed_all = false;
                break;
            }
        }
    }

    if ok {
        assert!(consumed_all && off == len, "valid input: the chain consumes it exactly");
    }
    if consumed_all && off == len {
        assert!(ok, "a fully consumed buffer is valid");
    }
}

/// The C ABI wrapper's boundary behaviour, which is the part neither the
/// standard library nor the pure functions above cover.
///
/// `len == 0` must answer "valid" without touching `src` - the C's contract
/// allows NULL there - and the decoder must answer 0 rather than reading.
#[cfg(feature = "core-utf8")]
#[kani::proof]
#[kani::unwind(4)]
fn c_abi_handles_empty_input() {
    unsafe {
        assert!(
            super::mkr_utf8_valid(core::ptr::null(), 0),
            "empty input is valid and src is not read"
        );
        let mut cp = 0u32;
        assert!(
            super::mkr_utf8_decode1(core::ptr::null(), 0, &mut cp) == 0,
            "empty input does not decode"
        );
        assert!(cp == 0, "a failed decode leaves *cp untouched");
    }
}
