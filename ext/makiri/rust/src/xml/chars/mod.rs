//! Pure character-level primitives: the XML 1.0 Char / Name classes, the strict
//! one-codepoint UTF-8 decoder and encoder, and §9.3's newline folding.
//!
//! Reference EXPANSION is [`expand`]: it is an engine with a cursor and its own
//! error domain, not a character class, and the two only shared a file.
//!
//! No unsafe code: every read is a slice index.

#![forbid(unsafe_code)]

pub mod expand;

use crate::falloc::Reserve;
use crate::xml::Status;
pub use expand::{expand_into, ExpandErr, ExpandMode};

/// XML 1.0 §2.2 Char.
#[inline]
pub fn is_char(c: u32) -> bool {
    c == 0x9
        || c == 0xA
        || c == 0xD
        || (0x20..=0xD7FF).contains(&c)
        || (0xE000..=0xFFFD).contains(&c)
        || (0x10000..=0x10FFFF).contains(&c)
}

/// XML 1.0 §2.3 NameStartChar.
#[inline]
pub fn is_name_start(c: u32) -> bool {
    c == b':' as u32
        || (b'A' as u32..=b'Z' as u32).contains(&c)
        || c == b'_' as u32
        || (b'a' as u32..=b'z' as u32).contains(&c)
        || (0xC0..=0xD6).contains(&c)
        || (0xD8..=0xF6).contains(&c)
        || (0xF8..=0x2FF).contains(&c)
        || (0x370..=0x37D).contains(&c)
        || (0x37F..=0x1FFF).contains(&c)
        || (0x200C..=0x200D).contains(&c)
        || (0x2070..=0x218F).contains(&c)
        || (0x2C00..=0x2FEF).contains(&c)
        || (0x3001..=0xD7FF).contains(&c)
        || (0xF900..=0xFDCF).contains(&c)
        || (0xFDF0..=0xFFFD).contains(&c)
        || (0x10000..=0xEFFFF).contains(&c)
}

/// XML 1.0 §2.3 NameChar.
#[inline]
pub fn is_name_char(c: u32) -> bool {
    is_name_start(c)
        || c == b'-' as u32
        || c == b'.' as u32
        || (b'0' as u32..=b'9' as u32).contains(&c)
        || c == 0xB7
        || (0x300..=0x36F).contains(&c)
        || (0x203F..=0x2040).contains(&c)
}

/// Decode ONE code point strictly (`cutf8::decode1`): truncation, bad
/// continuation bytes, overlong forms, surrogates and values above U+10FFFF are
/// all rejected.
///
/// Re-exported from `crate::cutf8` rather than written again here: there must be
/// exactly ONE strict decoder of ours, and `verify::accepted_is_utf8`
/// cross-checks it against `core::str::from_utf8` to keep it honest.
pub use crate::cutf8::decode1;

/// Every code point of `s` decodes strictly AND satisfies `ok`.
///
/// The decode-then-classify loop, once. Deliberately still built on this
/// module's own [`decode1`] rather than on `core::str::from_utf8`: `verify.rs`
/// proves properties of THIS traversal, and routing it through the standard
/// validator would make that proof a tautology.
#[inline]
fn all_code_points(s: &[u8], ok: impl Fn(u32) -> bool) -> bool {
    let mut i = 0;
    while i < s.len() {
        match decode1(&s[i..]) {
            Some((cp, bl)) if ok(cp) => i += bl,
            _ => return false,
        }
    }
    true
}

/// All of `s` is XML Char (no reference recognition).
pub fn validate_chars(s: &[u8]) -> bool {
    all_code_points(s, is_char)
}

/// `s` is a well-formed XML 1.0 Name (NameStartChar NameChar*). A colon is
/// permitted (this is the PITarget check).
pub fn validate_name(s: &[u8]) -> bool {
    let (cp, bl) = match decode1(s) {
        Some(x) => x,
        None => return false,
    };
    is_name_start(cp) && all_code_points(&s[bl..], is_name_char)
}

/// "xml" in any case (§2.6 reserved PITarget).
#[inline]
pub fn is_reserved_pi_target(s: &[u8]) -> bool {
    s.len() == 3 && s[0] | 0x20 == b'x' && s[1] | 0x20 == b'm' && s[2] | 0x20 == b'l'
}

/// One code point's UTF-8 bytes, as a value.
///
/// A value rather than a `(cp, &mut [u8; 4]) -> usize` out-parameter, which left
/// every caller re-deriving `&buf[..n]`; this mirrors `char::encode_utf8`.
pub struct Utf8Char {
    buf: [u8; 4],
    len: usize,
}

impl Utf8Char {
    /// Encode one code point (<= U+10FFFF).
    #[inline]
    pub fn encode(cp: u32) -> Utf8Char {
        let mut buf = [0u8; 4];
        let len = if cp < 0x80 {
            buf[0] = cp as u8;
            1
        } else if cp < 0x800 {
            buf[0] = 0xC0 | (cp >> 6) as u8;
            buf[1] = 0x80 | (cp & 0x3F) as u8;
            2
        } else if cp < 0x10000 {
            buf[0] = 0xE0 | (cp >> 12) as u8;
            buf[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            buf[2] = 0x80 | (cp & 0x3F) as u8;
            3
        } else {
            buf[0] = 0xF0 | (cp >> 18) as u8;
            buf[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
            buf[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            buf[3] = 0x80 | (cp & 0x3F) as u8;
            4
        };
        Utf8Char { buf, len }
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Fold CRLF and a lone CR to LF (§9.3b-A). Returns None when the input has
/// no CR (parse in place), else the normalized copy (which can only shrink).
///
/// The allocation is fallible, and its only failure mode is running out of
/// memory - so it says so, rather than returning a bare `Err(())` whose detail
/// the caller had to supply from somewhere else.
pub fn normalize_newlines(src: &[u8]) -> Result<Option<Vec<u8>>, Status> {
    if !src.contains(&b'\r') {
        return Ok(None);
    }
    let mut out: Vec<u8> = Vec::new();
    out.falloc_reserve_exact(src.len())
        .map_err(|_| Status::Oom)?;
    let mut i = 0;
    while i < src.len() {
        let ch = src[i];
        i += 1;
        if ch == b'\r' {
            out.push(b'\n');
            if src.get(i) == Some(&b'\n') {
                i += 1; /* CRLF -> single LF */
            }
        } else {
            out.push(ch);
        }
    }
    Ok(Some(out))
}
