//! Native Rust checks for the pure engine core.
//!
//! Kani proves the finite symbolic contracts under `cfg(kani)`.  These tests
//! complement that work with a normal `cargo test --no-default-features` run:
//! they are fast enough for every CI run, use independent standard-library
//! oracles where one exists, and preserve concrete regressions without
//! requiring a Ruby VM or a Lexbor build.

use crate::cutf8::{decode1, valid};
use crate::falloc::grow_capacity;
use crate::xml::chars::utf8_encode;
use crate::xml::qname::{is_enc_name, is_version_num, is_yes_no, split_checked, xmlns_prefix};
use crate::xpath::number::{extent, from_extent, to_text};

fn decoder_consumes_all(s: &[u8]) -> bool {
    let mut off = 0;
    while off < s.len() {
        match decode1(&s[off..]) {
            Some((_, n)) => off += n,
            None => return false,
        }
    }
    true
}

#[test]
fn utf8_decoder_agrees_with_the_standard_library_for_every_one_and_two_byte_input() {
    // A complete 65,793-input check.  This catches the lead-byte and
    // continuation-byte boundaries that examples tend to miss, while Kani
    // covers every possible four-byte decoder input separately.
    assert_eq!(valid(b""), decoder_consumes_all(b""));
    for first in 0u8..=u8::MAX {
        let one = [first];
        assert_eq!(valid(&one), decoder_consumes_all(&one), "{one:02x?}");
        for second in 0u8..=u8::MAX {
            let two = [first, second];
            assert_eq!(valid(&two), decoder_consumes_all(&two), "{two:02x?}");
        }
    }

    // Canonical 3- and 4-byte boundary cases, including every invalid class
    // the strict decoder promises to reject.
    for bytes in [
        "€".as_bytes(),
        "𐀀".as_bytes(),
        &[0xE0, 0x9F, 0x80],       // overlong three-byte sequence
        &[0xED, 0xA0, 0x80],       // surrogate
        &[0xF0, 0x8F, 0x80, 0x80], // overlong four-byte sequence
        &[0xF4, 0x90, 0x80, 0x80], // above U+10FFFF
        &[0xF0, 0x90, 0x80],       // truncated
    ] {
        assert_eq!(valid(bytes), decoder_consumes_all(bytes), "{bytes:02x?}");
    }
}

#[test]
fn utf8_encoder_matches_std_for_every_unicode_scalar() {
    for cp in 0..=0x10FFFF {
        let Some(ch) = char::from_u32(cp) else {
            continue; // surrogate code points are not Unicode scalars.
        };
        let mut actual = [0u8; 4];
        let n = utf8_encode(cp, &mut actual);
        assert_eq!(
            &actual[..n],
            ch.encode_utf8(&mut [0; 4]).as_bytes(),
            "U+{cp:04X}"
        );
    }
}

fn reference_number_extent(s: &[u8]) -> usize {
    enum State {
        Start,
        Integer(usize),
        PointAfterInteger(usize),
        Fraction(usize),
        PointAtStart,
    }

    let mut state = State::Start;
    for (i, &byte) in s.iter().enumerate() {
        state = match (state, byte) {
            (State::Start, b'0'..=b'9') => State::Integer(i + 1),
            (State::Start, b'.') => State::PointAtStart,
            (State::Integer(_), b'0'..=b'9') => State::Integer(i + 1),
            (State::Integer(_), b'.') => State::PointAfterInteger(i + 1),
            (State::PointAfterInteger(_), b'0'..=b'9') => State::Fraction(i + 1),
            (State::Fraction(_), b'0'..=b'9') => State::Fraction(i + 1),
            (State::PointAtStart, b'0'..=b'9') => State::Fraction(i + 1),
            (State::Integer(n) | State::PointAfterInteger(n) | State::Fraction(n), _) => return n,
            (State::Start | State::PointAtStart, _) => return 0,
        };
    }
    match state {
        State::Integer(n) | State::PointAfterInteger(n) | State::Fraction(n) => n,
        State::Start | State::PointAtStart => 0,
    }
}

fn each_number_candidate(prefix: &mut Vec<u8>, remaining: usize, f: &mut impl FnMut(&[u8])) {
    const ALPHABET: &[u8] = b".0123456789x";
    f(prefix);
    if remaining == 0 {
        return;
    }
    for &byte in ALPHABET {
        prefix.push(byte);
        each_number_candidate(prefix, remaining - 1, f);
        prefix.pop();
    }
}

#[test]
fn xpath_number_lexer_matches_an_independent_state_machine_exhaustively() {
    // 1 + 13 + ... + 13^5 = 402,234 short inputs.  The alphabet includes all
    // grammar characters plus a stopper, so this checks maximal munch as well
    // as acceptance without needing a property-test dependency.
    each_number_candidate(&mut Vec::new(), 5, &mut |s| {
        let n = extent(s);
        assert_eq!(n, reference_number_extent(s), "{s:?}");
        if n > 0 {
            let text = core::str::from_utf8(&s[..n]).unwrap();
            assert_eq!(from_extent(&s[..n]), text.parse::<f64>().unwrap(), "{s:?}");
        }
    });
}

#[test]
fn xpath_number_rendering_has_xpath_boundary_behaviour() {
    let cases = [
        (0.0, "0"),
        (-0.0, "0"),
        (1.0, "1"),
        (-1.0, "-1"),
        (1.5, "1.5"),
        (0.0001, "0.0001"),
        (0.00001, "1e-05"),
        (1e15, "1e+15"),
    ];
    for (number, expected) in cases {
        let mut out = [0u8; 64];
        let n = to_text(number, &mut out).expect("sufficient fixed buffer");
        assert_eq!(&out[..n], expected.as_bytes());
    }
    assert!(to_text(1.5, &mut [0u8; 1]).is_none());
}

#[test]
fn qname_and_declaration_grammars_reject_boundary_forms() {
    assert!(split_checked(b"root").is_some());
    assert!(split_checked("p:要素".as_bytes()).is_some());
    for bad in [
        b"".as_slice(),
        b":root",
        b"root:",
        b"a:b:c",
        b"9root",
        b"\xC0\x80",
    ] {
        assert!(split_checked(bad).is_none(), "{bad:02x?}");
    }
    assert_eq!(xmlns_prefix(b"xmlns"), Some(b"".as_slice()));
    assert_eq!(xmlns_prefix(b"xmlns:svg"), Some(b"svg".as_slice()));
    assert_eq!(xmlns_prefix(b"xmlnsx"), None);
    assert!(is_version_num(b"1.0"));
    assert!(!is_version_num(b"1."));
    assert!(is_enc_name(b"UTF-8"));
    assert!(!is_enc_name(b"8UTF"));
    assert!(is_yes_no(b"yes"));
    assert!(is_yes_no(b"no"));
    assert!(!is_yes_no(b"Yes"));
}

#[test]
fn growth_policy_never_returns_an_unallocatable_or_insufficient_capacity() {
    let sizes = [
        0,
        1,
        7,
        8,
        9,
        63,
        64,
        65,
        usize::MAX / 2,
        usize::MAX - 1,
        usize::MAX,
    ];
    let elements = [1, 2, 8, usize::MAX / 2, usize::MAX];
    for cap in sizes {
        for need in sizes {
            for elem in elements {
                match grow_capacity(cap, need, elem) {
                    Some(next) => {
                        assert!(next >= need, "cap={cap} need={need} elem={elem}");
                        assert!(
                            next.checked_mul(elem).is_some(),
                            "cap={cap} need={need} elem={elem} next={next}"
                        );
                    }
                    None => assert!(need.checked_mul(elem).is_none()),
                }
            }
        }
    }
}
