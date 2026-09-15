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

#[test]
fn growth_policy_returns_zero_for_empty_request() {
    // `need == 0` is a contract exception: no allocation is required, so the
    // answer is always 0 regardless of the current capacity. This was the case
    // Kani's "never shrink" assertion stumbled over.
    for cap in [0, 1, 8, usize::MAX / 8, usize::MAX] {
        for elem in [1, 8, usize::MAX] {
            assert_eq!(
                grow_capacity(cap, 0, elem),
                Some(0),
                "cap={cap} elem={elem}"
            );
        }
    }
}

#[test]
fn growth_policy_never_shrinks_a_live_allocation_for_non_empty_need() {
    // A "live" cap has a non-zero byte size that fits. For any non-empty need
    // not larger than that cap, the answer must stay at least as large as cap.
    // (For need == 0 see the separate empty-request test.)
    for cap in [1, 8, 64, usize::MAX / 8] {
        for need in [1usize, cap, cap.saturating_mul(2)] {
            if need == 0 {
                continue;
            }
            for elem in [1, 8] {
                let Some(next) = grow_capacity(cap, need, elem) else {
                    // `need` itself did not fit; the function is allowed to fail.
                    continue;
                };
                assert!(next >= cap, "cap={cap} need={need} elem={elem} next={next}");
            }
        }
    }
}

#[test]
fn node_id_tokens_fail_closed_outside_their_document() {
    // A node-set token is not authenticated, so the checked accessor must
    // reject anything that does not name a live slot in THIS document: a
    // foreign document's handle (same index, different stamp), an out-of-range
    // index, and the null handle.
    use crate::xml::{Document, NodeId, NodeType};

    let mut a = Document::create(None, 0).expect("doc a");
    let mut b = Document::create(None, 0).expect("doc b");
    let na = a.new_node(NodeType::Element).expect("node a");
    let nb = b.new_node(NodeType::Element).expect("node b");

    // The handle resolves in its own document.
    assert_eq!(a.try_node(na).map(|n| n.type_), Some(NodeType::Element));
    assert_eq!(b.try_node(nb).map(|n| n.type_), Some(NodeType::Element));

    // Same slot index, different document stamp -> rejected.
    assert_eq!(na.index(), nb.index());
    assert!(b.try_node(na).is_none());
    assert!(a.try_node(nb).is_none());

    // Out-of-range index -> rejected, not a panic.
    let oob = NodeId::new(u32::MAX - 1, na.stamp());
    assert!(a.try_node(oob).is_none());

    // The null handle -> rejected.
    assert!(a.try_node(NodeId::INVALID).is_none());
    assert!(NodeId::INVALID.is_invalid());
}

#[test]
fn text_verdict_accepts_valid_utf8() {
    use crate::cutf8::{text_verdict, TextVerdict};
    assert_eq!(text_verdict(b"", false), TextVerdict::Ok);
    assert_eq!(text_verdict(b"hello", false), TextVerdict::Ok);
    assert_eq!(text_verdict("日本語".as_bytes(), false), TextVerdict::Ok);
}

#[test]
fn text_verdict_rejects_nul_before_utf8() {
    // NUL is well-formed UTF-8, but the strict contract forbids it, so it is
    // reported as HasNul even when the bytes are otherwise valid, and even when
    // a caller has already proved the bytes valid UTF-8.
    use crate::cutf8::{text_verdict, TextVerdict};
    assert_eq!(text_verdict(b"a\0b", false), TextVerdict::HasNul);
    assert_eq!(text_verdict(b"a\0b", true), TextVerdict::HasNul);
}

#[test]
fn text_verdict_rejects_invalid_utf8() {
    use crate::cutf8::{text_verdict, TextVerdict};
    assert_eq!(text_verdict(b"\xFF", false), TextVerdict::InvalidUtf8);
    // A truncated multi-byte sequence.
    assert_eq!(text_verdict(b"\xE2\x82", false), TextVerdict::InvalidUtf8);
    // An overlong encoding of '/' (0xC0 0xAF) is not well-formed.
    assert_eq!(text_verdict(b"\xC0\xAF", false), TextVerdict::InvalidUtf8);
}

#[test]
fn text_verdict_skips_the_scan_when_already_known_valid() {
    // A caller that has proved validity (a whole-string coderange) skips the
    // UTF-8 scan, so bytes the scan would reject are accepted - the
    // BOM-stripped-suffix case - while the NUL rule still applies.
    use crate::cutf8::{text_verdict, TextVerdict};
    assert_eq!(text_verdict(b"\xFF", true), TextVerdict::Ok);
    assert_eq!(text_verdict(b"\xFF\0", true), TextVerdict::HasNul);
}

#[test]
fn text_verdict_agrees_with_the_standard_library_on_every_one_and_two_byte_input() {
    // The verdict is "well-formed UTF-8 and no NUL", which `str::from_utf8`
    // answers for the whole 1- and 2-byte domain - the boundary-rich part. A
    // standalone oracle, not the validator under test.
    use crate::cutf8::{text_verdict, TextVerdict};
    let oracle = |b: &[u8]| -> TextVerdict {
        if b.contains(&0) {
            TextVerdict::HasNul
        } else if core::str::from_utf8(b).is_ok() {
            TextVerdict::Ok
        } else {
            TextVerdict::InvalidUtf8
        }
    };
    for first in 0u8..=u8::MAX {
        let one = [first];
        assert_eq!(text_verdict(&one, false), oracle(&one), "{one:02x?}");
        for second in 0u8..=u8::MAX {
            let two = [first, second];
            assert_eq!(text_verdict(&two, false), oracle(&two), "{two:02x?}");
        }
    }
}

#[test]
fn verified_text_rejects_nul_and_invalid_utf8() {
    use crate::text::VerifiedText;
    assert!(VerifiedText::from_bytes(b"a\0b").is_none());
    assert!(VerifiedText::from_bytes(b"\xFF").is_none());

    let bytes = "日本語".as_bytes();
    let t = VerifiedText::from_bytes(bytes).unwrap();
    // A borrow, not a copy.
    assert_eq!(t.as_ptr() as *const u8, bytes.as_ptr());
    assert_eq!(t.len(), bytes.len());
    assert_eq!(unsafe { t.as_bytes() }, bytes);
}

#[test]
fn text_views_distinguish_absent_from_empty() {
    use crate::text::{BorrowedText, VerifiedText};

    let absent = unsafe { BorrowedText::from_raw_parts(core::ptr::null(), 0) };
    assert!(absent.is_absent() && absent.is_empty());
    assert!(unsafe { absent.as_bytes() }.is_empty());

    // `empty` is present and NUL-terminated, so it is safe wherever a present
    // string is required.
    let empty = VerifiedText::empty();
    assert!(!empty.is_absent() && empty.is_empty());
    assert_eq!(unsafe { *empty.as_ptr() }, 0);

    let present = VerifiedText::from_bytes(b"").unwrap();
    assert!(!present.is_absent() && present.is_empty());

    // Weakening to a borrowed view keeps presence and the bytes.
    let b: BorrowedText = empty.into();
    assert!(!b.is_absent() && b.is_empty());
    let v = VerifiedText::from_bytes(b"xy").unwrap();
    let b: BorrowedText = v.into();
    assert_eq!((b.as_ptr(), b.len()), (v.as_ptr(), v.len()));
}

#[test]
fn borrowed_text_carries_an_interior_nul() {
    use crate::text::BorrowedText;
    let data = b"a\0b";
    let b = unsafe { BorrowedText::from_raw_parts(data.as_ptr() as *const _, data.len()) };
    assert_eq!(b.len(), 3);
    assert_eq!(unsafe { b.as_bytes() }, data);
}

#[test]
fn owned_text_copy_keeps_interior_nul_and_terminates() {
    use crate::text::BorrowedText;
    use crate::xpath::msg::ErrSink;
    use crate::xpath::value::TextSlot;
    use core::ptr;

    let mut t =
        unsafe { TextSlot::try_copy_bytes(b"a\0b", ErrSink::silent(), None) }.expect("allocation");
    assert_eq!(t.len(), 3);
    assert_eq!(unsafe { t.as_bytes() }, b"a\0b");
    assert_eq!(unsafe { *t.as_ptr().add(3) }, 0);
    unsafe { t.clear() };
    assert!(t.is_absent());

    // An absent view copies to a present empty string, not to another absent.
    let mut e = unsafe {
        TextSlot::try_copy(
            BorrowedText::from_raw_parts(ptr::null(), 0),
            ErrSink::silent(),
            None,
        )
    }
    .expect("allocation");
    assert!(e.is_present() && e.is_empty());
    assert_eq!(unsafe { *e.as_ptr() }, 0);
    unsafe { e.clear() };
}

#[test]
fn owned_text_adopts_a_detached_buffer() {
    use crate::cbuf::Buf;
    use crate::xpath::value::TextSlot;

    let mut buf = Buf::new(0);
    buf.append(b"a\0b").expect("append");
    let mut t = TextSlot::from_buf(buf.steal().expect("steal"));
    assert_eq!(unsafe { t.as_bytes() }, b"a\0b");
    assert_eq!(unsafe { *t.as_ptr().add(3) }, 0);
    unsafe { t.clear() };
}

#[test]
fn owned_text_fill_terminates_at_the_length_written() {
    use crate::xpath::value::TextSlot;

    let mut t = TextSlot::try_fill(5, |dst| {
        // The reservation arrives zeroed.
        assert!(dst.iter().all(|&b| b == 0));
        dst[..2].copy_from_slice(b"ab");
        2
    })
    .expect("allocation");
    assert_eq!(unsafe { t.as_bytes() }, b"ab");
    assert_eq!(unsafe { *t.as_ptr().add(2) }, 0);
    unsafe { t.clear() };

    let mut e = TextSlot::try_fill(0, |_| 0).expect("allocation");
    assert!(e.is_present() && e.is_empty());
    unsafe { e.clear() };
}

#[test]
fn xml_serialization_answers_what_the_ruby_methods_did_and_round_trips() {
    use crate::xml::parse::mkr_xml_parse;
    use crate::xml::serialize::{canonicalize, to_xml};

    // The expected bytes are what `#to_xml` and `#canonicalize` answered before
    // the serializer moved out of the glue.
    let src = br#"<?xml version="1.0"?><r xmlns:p="urn:p" b="2" a="1"><p:x>t &amp; u</p:x><!--c--><e/></r>"#;
    let doc = mkr_xml_parse(src).expect("well-formed");
    let top = doc.doc_node();
    let root = doc.root.expect("a root element");

    let whole = to_xml(&doc, top, 0, None).expect("serializes");
    assert_eq!(
        whole.as_slice(),
        b"<?xml version=\"1.0\"?>\n<r xmlns:p=\"urn:p\" b=\"2\" a=\"1\"><p:x>t &amp; u</p:x><!--c--><e/></r>\n"
    );
    assert_eq!(
        to_xml(&doc, root, 2, None).expect("serializes").as_slice(),
        b"<r xmlns:p=\"urn:p\" b=\"2\" a=\"1\">\n  <p:x>t &amp; u</p:x>\n  <!--c-->\n  <e/>\n</r>"
    );
    assert_eq!(
        canonicalize(&doc, top, false)
            .expect("canonicalizes")
            .as_slice(),
        b"<r xmlns:p=\"urn:p\" a=\"1\" b=\"2\"><p:x>t &amp; u</p:x><e></e></r>"
    );
    assert_eq!(
        canonicalize(&doc, top, true)
            .expect("canonicalizes")
            .as_slice(),
        b"<r xmlns:p=\"urn:p\" a=\"1\" b=\"2\"><p:x>t &amp; u</p:x><!--c--><e></e></r>"
    );

    // The output re-parses to a tree that serializes to the same bytes.
    let again = mkr_xml_parse(whole.as_slice()).expect("output re-parses");
    assert_eq!(
        to_xml(&again, again.doc_node(), 0, None)
            .expect("serializes")
            .as_slice(),
        whole.as_slice()
    );

    // A declared encoding is kept, and a requested one is declared.
    let declared =
        mkr_xml_parse(br#"<?xml version="1.0" encoding="UTF-8"?><a/>"#).expect("well-formed");
    let expected: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a/>\n";
    assert_eq!(
        to_xml(&declared, declared.doc_node(), 0, None)
            .expect("serializes")
            .as_slice(),
        expected
    );
    let plain = mkr_xml_parse(b"<a/>").expect("well-formed");
    assert_eq!(
        to_xml(&plain, plain.doc_node(), 0, Some(b"UTF-8"))
            .expect("serializes")
            .as_slice(),
        expected
    );
}
