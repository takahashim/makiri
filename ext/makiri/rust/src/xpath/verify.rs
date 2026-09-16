//! Kani proofs for the XPath Number production.
//!
//! These replace `verify/harness_xpath_number.c`. The replacement is NOT
//! one-for-one, and the difference is the interesting part - see
//! notes/rust_port_remaining.ja.md step 4.
//!
//! The C harness's subject was the conversion. `strtod` accepts a superset of
//! the production and honours `LC_NUMERIC`, so a comma-decimal locale would
//! misread "1.5"; the C carried a hand-rolled digit assembler as the fallback,
//! and `cbmc_models.c` returned "conversion unavailable" on every input
//! precisely to steer execution into it. **That code does not exist in Rust.**
//! `f64::from_str` is locale-independent and correctly rounded, so the fallback
//! was deleted rather than ported, and a proof of it has nothing left to say.
//!
//! What remains ours is the grammar gate, and the contract it owes the parser.
//! Both are proved below. The parser itself is not proved here and should not
//! be: Kani does not close through `str::parse::<f64>()` (measured: no result
//! at 4 nondet bytes after 17 minutes), and std's float parser is not our code.

#![forbid(unsafe_code)]
#![cfg(kani)]

use crate::kani_bounds::parse_usize;

use super::number::extent;

/// The longest input these proofs quantify over.
///
/// Six, not eight: the Number production is a flat scan, so the interesting
/// shapes (digits, one point, a bare point, a trailing non-digit) all fit, and
/// nothing about the grammar becomes reachable only at greater length. The
/// cross-implementation UTF-8 proof next door needs eight because a chained
/// multibyte decode does.
///
/// Raise it with `KANI_XPATH_NUMBER_MAX`.
const N: usize = match option_env!("KANI_XPATH_NUMBER_MAX") {
    Some(s) => parse_usize(s),
    None => 6,
};

/// `extent` returns the longest prefix matching `Digits ('.' Digits?)? | '.' Digits`.
///
/// Three things at once, because they are one property: what it accepts is in
/// the production, it never runs past the slice, and it is maximal - a shorter
/// answer would leave the lexer to restart mid-number.
#[kani::proof]
#[kani::unwind(8)]
fn extent_matches_the_grammar() {
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let s = &buf[..len];

    let n = extent(s);
    assert!(n <= s.len(), "the extent must stay inside the slice");
    if n == 0 {
        return;
    }

    let mut dots = 0usize;
    let mut digits = 0usize;
    let mut i = 0usize;
    while i < n {
        let b = s[i];
        if b == b'.' {
            dots += 1;
        } else {
            assert!(b.is_ascii_digit(), "a Number is digits and at most one '.'");
            digits += 1;
        }
        i += 1;
    }
    assert!(dots <= 1, "at most one decimal point");
    assert!(digits >= 1, "a bare '.' is not a Number");

    // Maximal munch: the byte after the extent could not have extended it.
    if n < s.len() {
        let next = s[n];
        assert!(
            !next.is_ascii_digit(),
            "stopping before a digit is not maximal"
        );
        if next == b'.' {
            assert!(dots == 1, "stopping before the FIRST '.' is not maximal");
        }
    }
}

/// The handover contract: what the gate accepts is exactly what the parser
/// takes, so the conversion downstream cannot fail.
///
/// This is deliberately expressed as a property of OUR output rather than by
/// running the parser. `f64::from_str` accepts a decimal with optional sign,
/// optional fraction and optional exponent; the gate emits a strict subset of
/// that (no sign, no exponent, at most one point, at least one digit), and
/// those are ASCII, so `from_utf8` cannot fail either. Proving our side and
/// naming std's is honest about where the boundary of the proof is - running
/// the parser inside Kani would not close, and would not be verifying our code
/// if it did.
#[kani::proof]
#[kani::unwind(8)]
fn accepted_bytes_satisfy_the_parser_precondition() {
    let buf: [u8; N] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= N);
    let s = &buf[..len];

    let n = extent(s);
    if n == 0 {
        return;
    }
    let accepted = &s[..n];

    let mut i = 0usize;
    while i < accepted.len() {
        let b = accepted[i];
        // ASCII, so the UTF-8 decode in from_extent cannot fail.
        assert!(b < 0x80, "the gate must only emit ASCII");
        // In the subset f64::from_str accepts.
        assert!(b.is_ascii_digit() || b == b'.', "digits and '.' only");
        // No sign and no exponent: the production carries neither, so the
        // parsed value can never be negative or lose precision to an exponent.
        assert!(b != b'-' && b != b'+' && b != b'e' && b != b'E');
        i += 1;
    }
}
