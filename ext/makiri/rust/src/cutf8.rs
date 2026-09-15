//! The shared UTF-8 primitives (core/mkr_utf8.c).
//!
//! Two functions, deliberately implemented DIFFERENTLY from each other:
//!
//! - [`valid`] is `core::str::from_utf8`. The C hand-wrote the Unicode
//!   well-formed table with a word-at-a-time ASCII fast path; the standard
//!   library does the same job, with the same acceptance set, and brings the
//!   memory safety and the range guarantees with it.
//! - [`decode1`] stays OURS, a direct port of the C's strict one-codepoint
//!   decoder.
//!
//! # Why not route both through `core::str`
//!
//! Because the property worth keeping is that the two AGREE, and agreement is
//! only evidence when the two sides are written differently. The C proved it
//! with CBMC by cross-checking its own decoder against its own table scan;
//! routing both through the standard library would turn that into a
//! restatement. `crate::xml::verify` already carries the same warning about
//! `validate_chars`, written before this module existed.
//!
//! The Kani proofs in [`verify`] are what that cross-check became.
//!
//! # What the CBMC proofs covered, and where each part went
//!
//! `verify/harness_utf8.c`, `harness_utf8_chain.c` and `harness_utf8_words.c`
//! prove things about `core/mkr_utf8.c`. They still run, and still pass, but
//! they cover the C build - so this is the accounting for the Rust one:
//!
//! - **memory safety** (`--bounds-check` over the scan): gone as an obligation.
//!   Both functions take a slice; there is no pointer arithmetic to get wrong.
//! - **the word-at-a-time scan** (`harness_utf8_words.c`): the subject no
//!   longer exists. `from_utf8` has its own fast path, which is not ours.
//! - **scalar range, no surrogates, no overlongs**: `from_utf8`'s contract for
//!   the validator; proved for the decoder in [`verify::decode1_is_strict`].
//! - **decoder/validator agreement, first code point and whole buffer**:
//!   [`verify::decode1_agrees_with_from_utf8`] and
//!   [`verify::chain_consumes_exactly_valid_input`].

pub mod verify;

/// Decode ONE code point from the front of `p`, strictly.
///
/// Rejects truncation, bad continuation bytes, overlong forms, surrogates and
/// values above U+10FFFF. `Some((codepoint, byte_length))` or `None` - fail
/// closed, never reading past the slice.
///
/// The one strict decoder in the crate: the XML tokenizer's name and Char
/// scanning and the XPath lexer both reach it through `xml::chars`, which
/// re-exports this rather than keeping a second copy.
#[inline]
pub fn decode1(p: &[u8]) -> Option<(u32, usize)> {
    let b0 = *p.first()? as u32;
    if b0 < 0x80 {
        return Some((b0, 1));
    }
    let (len, min, init) = if b0 & 0xE0 == 0xC0 {
        (2usize, 0x80u32, b0 & 0x1F)
    } else if b0 & 0xF0 == 0xE0 {
        (3, 0x800, b0 & 0x0F)
    } else if b0 & 0xF8 == 0xF0 {
        (4, 0x10000, b0 & 0x07)
    } else {
        /* A continuation byte or an 0xF8+ lead: never starts a code point. */
        return None;
    };
    let tail = p.get(1..len)?; /* None when truncated */
    let mut cp = init;
    for &b in tail {
        if b & 0xC0 != 0x80 {
            return None;
        }
        cp = (cp << 6) | (b as u32 & 0x3F);
    }
    if cp < min || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
        return None;
    }
    Some((cp, len))
}

/// Whether `s` is well-formed UTF-8 (RFC 3629 / WHATWG).
///
/// A NUL byte is VALID here - U+0000 is well-formed UTF-8 - and callers that
/// must reject it check separately. That matches the C's contract exactly.
#[inline]
pub fn valid(s: &[u8]) -> bool {
    core::str::from_utf8(s).is_ok()
}

/// The strict-text verdict the name/engine boundary enforces over already-
/// resolved bytes: NUL, then well-formed UTF-8.
///
/// This is the whole check behind `bridge::string::text_check`, lifted here
/// (Ruby-free, Lexbor-free) so the logic is a plain function rather than an
/// `unsafe` one and is testable under the always-compiled core - the
/// `cargo test --no-default-features` set. The bridge keeps a thin `unsafe`
/// wrapper that resolves the raw pointer and the String's cached coderange into
/// the two ordinary arguments:
///
///   * `bytes`: the byte range to validate.
///   * `known_valid_utf8`: whether the bytes are ALREADY known valid UTF-8
///     (typically read from a Ruby String's cached coderange). When true the
///     UTF-8 scan is skipped; the NUL search still runs, because NUL is valid
///     UTF-8 but the strict contract forbids it.
///
/// `known_valid_utf8` may be established from a SUPERSTRING of `bytes` (the XML
/// path knows the whole decoded String is valid but validates a BOM-stripped
/// suffix): a whole-string VALID coderange proves any suffix valid, because the
/// BOM is one complete UTF-8 character.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextVerdict {
    /// Valid UTF-8 with no interior NUL.
    Ok,
    /// Valid UTF-8 but containing a NUL, which names and engine inputs forbid.
    HasNul,
    /// Not well-formed UTF-8.
    InvalidUtf8,
}

#[inline]
pub fn text_verdict(bytes: &[u8], known_valid_utf8: bool) -> TextVerdict {
    if bytes.contains(&0) {
        return TextVerdict::HasNul;
    }
    if known_valid_utf8 || valid(bytes) {
        return TextVerdict::Ok;
    }
    TextVerdict::InvalidUtf8
}
