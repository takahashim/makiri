//! Kani proofs for the XML character layer.
//!
//! These replace `verify/harness_xml_chars.c`, whose subject
//! (`xml/mkr_xml_chars.c`) the Rust build stopped compiling long before the file
//! was deleted - and `rake verify` kept passing over it the whole time, proving
//! a translation unit nothing executed. That is why the correspondence below is
//! written down rather than assumed: a proof is only evidence about the code
//! that actually runs.
//!
//! Run with `rake kani` (or `cargo kani --features xml,xpath`).

#![cfg(kani)]

use crate::kani_bounds::parse_usize;

use super::chars::{decode1, is_char, is_name_char, is_name_start, validate_chars};

/// The longest input these proofs quantify over.
///
/// Eight matches the bound the retired CBMC harness used
/// (`VERIFY_XML_CHARS_MAX`), so the guarantee did not shrink when it changed
/// languages. The cost grows fast - measured 11s at 4 bytes, 26s at 6, 57s at
/// 8 - and a minute is what a verification job can pay.
///
/// Raise it with `KANI_XML_CHARS_MAX` when a deeper run is worth the wait:
/// `KANI_XML_CHARS_MAX=10 bundle exec rake kani`.
const N: usize = match option_env!("KANI_XML_CHARS_MAX") {
    Some(s) => parse_usize(s),
    None => 8,
};

/// The predicate inclusions, over EVERY u32 - not sampled.
///
/// NameStartChar is a subset of NameChar, and NameChar of Char (XML 1.0
/// §2.2/§2.3). This is the same property the C harness proved and at the same
/// strength, since both quantify over the whole 32-bit space.
#[kani::proof]
fn predicate_inclusions() {
    let c: u32 = kani::any();
    if is_name_start(c) {
        assert!(is_name_char(c), "NameStartChar must be a NameChar");
    }
    if is_name_char(c) {
        assert!(is_char(c), "NameChar must be a Char");
    }
}

/// The load-bearing one: bytes `validate_chars` accepts are valid UTF-8
/// according to an **independent** implementation.
///
/// The C version cross-checked our strict one-codepoint decoder against our
/// word-at-a-time table scan, and the value was in the two being written
/// differently - agreement is evidence, not a restatement. Kani sees only Rust,
/// and the word-at-a-time scan is still C (`core/mkr_utf8.c`), so the second
/// implementation here is `core::str::from_utf8`.
///
/// That preserves the property's character, and arguably improves it: the
/// second implementation is now one we did not write at all. When `core/` moves
/// to Rust (step 8), this proof is the reason not to route `validate_chars`
/// through the same validator - doing so would turn it into a tautology.
#[kani::proof]
#[kani::unwind(10)]
fn accepted_is_utf8() {
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let s = &buf[..len];
    if validate_chars(s) {
        assert!(core::str::from_utf8(s).is_ok(), "accepted bytes must be UTF-8");
    }
}

/// The decoder's length contract.
///
/// Reading past the slice is not in question here the way it was in C - the
/// borrow checker settles it - but `validate_chars` advances by the reported
/// length, so a length of 0 would loop forever and a length past the end would
/// skip input. Neither is a memory-safety property, so neither comes free.
#[kani::proof]
#[kani::unwind(10)]
fn decode1_length_and_range() {
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let s = &buf[..len];
    if let Some((cp, bl)) = decode1(s) {
        assert!(bl >= 1, "a zero-length decode would make validate_chars loop");
        assert!(bl <= s.len(), "the consumed length must stay inside the slice");
        assert!(cp <= 0x10FFFF, "a decoded value must be a Unicode code point");
        assert!(!(0xD800..=0xDFFF).contains(&cp), "surrogates must be rejected");
    }
}
