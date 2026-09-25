//! The shared UTF-8 primitives.
//!
//! Two functions:
//!
//! - [`valid`] is `core::str::from_utf8`. The C hand-wrote the Unicode
//!   well-formed table with a word-at-a-time ASCII fast path; the standard
//!   library does the same job, with the same acceptance set, and brings the
//!   memory safety and the range guarantees with it.
//! - [`decode1`] stays OURS: a strict one-codepoint decoder.
//!
//! # Why `decode1` is not `core::str` too
//!
//! Performance, on a path that runs once per character. Stable std has no
//! "decode the first code point of these bytes" - the nearest is
//! `from_utf8(&p[..width])` then `.chars().next()`, which was measured and
//! rejected: XML parsing of Japanese text took ~11% longer. `from_utf8` is
//! built for long input (an alignment computation and a 16-byte ASCII block
//! loop before the first multi-byte check, then a `Result` to build), is too
//! large for LLVM to inline at every call site even under LTO, and leaves
//! `.chars()` to decode the same bytes a second time. This decoder validates
//! and assembles the code point in one pass.
//!
//! It gives up nothing in correctness: [`verify::decode1_agrees_with_from_utf8`]
//! proves it returns exactly what `from_utf8` accepts, for every input the
//! bound covers. Keep that proof with it - it is what makes a hand-written
//! decoder as trustworthy as the standard one.
//!
//! # What the CBMC proofs covered, and where each part went
//!
//! `verify/harness_utf8.c`, `harness_utf8_chain.c` and `harness_utf8_words.c`
//! proved things about the C validator, and went with it. This is the
//! accounting of where each property went:
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

#![forbid(unsafe_code)]

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
    decode_multibyte(p, b0)
}

/// [`decode1`] past the ASCII fast path: `b0` is `p[0]`, and >= 0x80.
///
/// Out of line on purpose. It keeps `decode1` down to the ASCII test and a
/// call, small enough to inline into every per-character loop, which measured
/// ~10% on ASCII-heavy XML parsing and nothing lost on Japanese text.
#[inline(never)]
fn decode_multibyte(p: &[u8], b0: u32) -> Option<(u32, usize)> {
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
    /* `from_u32` refuses exactly what is not a scalar value: past U+10FFFF,
     * or a surrogate. */
    if cp < min || char::from_u32(cp).is_none() {
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

impl TextVerdict {
    /// What is wrong, as the predicate of an error message - "<subject> must be
    /// valid UTF-8" - or `None` for [`TextVerdict::Ok`]. The one wording every
    /// error surface uses (`Makiri::Error`, `XML::SyntaxError`, a reason string).
    pub fn problem(self) -> Option<&'static str> {
        match self {
            TextVerdict::Ok => None,
            TextVerdict::HasNul => Some("must not contain a NUL byte"),
            TextVerdict::InvalidUtf8 => Some("must be valid UTF-8"),
        }
    }

    /// What the DATA contract rejects - the HTML data family (text, comment
    /// and attribute values) may hold U+0000 like browsers, so only invalid
    /// UTF-8 is a problem there.
    pub fn data_problem(self) -> Option<&'static str> {
        match self {
            TextVerdict::InvalidUtf8 => self.problem(),
            TextVerdict::Ok | TextVerdict::HasNul => None,
        }
    }

    /// [`problem`](Self::problem) with "string" as its subject, for a caller
    /// that reports through a static string.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            TextVerdict::Ok => None,
            TextVerdict::HasNul => Some("string must not contain a NUL byte"),
            TextVerdict::InvalidUtf8 => Some("string must be valid UTF-8"),
        }
    }
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
