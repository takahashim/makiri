//! Pure character-level primitives: XML 1.0 Char / Name classes, the strict
//! one-codepoint UTF-8 decoder, and entity / character-reference expansion.
//! No unsafe code: every read is a slice index, every write goes through the
//! bounded `Writer`.

/* `normalize_newlines` reports failure as `Err(())`: the detail is the parser's
 * status code, which the caller already holds, exactly as in the C. */
#![allow(clippy::result_unit_err)]

#![forbid(unsafe_code)]

use crate::falloc::Reserve;

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

/// Decode ONE code point strictly (mkr_utf8_decode1): truncation, bad
/// continuation bytes, overlong forms, surrogates and values above U+10FFFF
/// all yield None. Never reads past the slice.
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
        return None;
    };
    let tail = p.get(1..len)?;
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

/// All of `s` is XML Char (no reference recognition). mkr_xml_validate_chars.
pub fn validate_chars(s: &[u8]) -> bool {
    let mut i = 0;
    while i < s.len() {
        match decode1(&s[i..]) {
            Some((cp, bl)) if is_char(cp) => i += bl,
            _ => return false,
        }
    }
    true
}

/// `s` is a well-formed XML 1.0 Name (NameStartChar NameChar*). A colon is
/// permitted (this is the PITarget check). mkr_xml_validate_name.
pub fn validate_name(s: &[u8]) -> bool {
    let (cp, bl) = match decode1(s) {
        Some(x) => x,
        None => return false,
    };
    if !is_name_start(cp) {
        return false;
    }
    let mut i = bl;
    while i < s.len() {
        match decode1(&s[i..]) {
            Some((cp, bl)) if is_name_char(cp) => i += bl,
            _ => return false,
        }
    }
    true
}

/// "xml" in any case (§2.6 reserved PITarget).
#[inline]
pub fn is_reserved_pi_target(s: &[u8]) -> bool {
    s.len() == 3 && s[0] | 0x20 == b'x' && s[1] | 0x20 == b'm' && s[2] | 0x20 == b'l'
}

/// Encode one code point (<= U+10FFFF) into `out`; returns the byte length.
#[inline]
pub fn utf8_encode(cp: u32, out: &mut [u8; 4]) -> usize {
    if cp < 0x80 {
        out[0] = cp as u8;
        1
    } else if cp < 0x800 {
        out[0] = 0xC0 | (cp >> 6) as u8;
        out[1] = 0x80 | (cp & 0x3F) as u8;
        2
    } else if cp < 0x10000 {
        out[0] = 0xE0 | (cp >> 12) as u8;
        out[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        out[2] = 0x80 | (cp & 0x3F) as u8;
        3
    } else {
        out[0] = 0xF0 | (cp >> 18) as u8;
        out[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
        out[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        out[3] = 0x80 | (cp & 0x3F) as u8;
        4
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExpandMode {
    /// Copy content verbatim (no whitespace folding).
    Text,
    /// XML 1.0 §3.3.3: a LITERAL TAB/LF/CR becomes a space; a reference-derived
    /// one is preserved.
    Attr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExpandErr {
    /// Undefined entity, malformed reference, or a non-XML-Char.
    Syntax,
    /// The output would exceed the caller's buffer ("output <= input" broke -
    /// an internal invariant violation, never reachable from input).
    Overflow,
}

/// Bounded writer over a caller-provided buffer: a write past the end is
/// refused (Err), never performed.
struct Writer<'a> {
    out: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    #[inline]
    fn put(&mut self, b: u8) -> Result<(), ExpandErr> {
        match self.out.get_mut(self.pos) {
            Some(slot) => {
                *slot = b;
                self.pos += 1;
                Ok(())
            }
            None => Err(ExpandErr::Overflow),
        }
    }
    #[inline]
    fn put_slice(&mut self, s: &[u8]) -> Result<(), ExpandErr> {
        let end = self.pos.checked_add(s.len()).ok_or(ExpandErr::Overflow)?;
        match self.out.get_mut(self.pos..end) {
            Some(dst) => {
                dst.copy_from_slice(s);
                self.pos = end;
                Ok(())
            }
            None => Err(ExpandErr::Overflow),
        }
    }
}

/// Expand the 5 predefined entities + numeric character references in `src`
/// into `out` (which must hold at least `src.len()` bytes - the output is never
/// longer than the input), validating XML Char and, in Attr mode, folding
/// literal whitespace. Returns the number of bytes written. mkr_xml_expand's
/// pure core.
pub fn expand_into(src: &[u8], mode: ExpandMode, out: &mut [u8]) -> Result<usize, ExpandErr> {
    let mut w = Writer { out, pos: 0 };
    let mut i = 0usize;
    while i < src.len() {
        if src[i] != b'&' {
            let (cp, bl) = decode1(&src[i..]).ok_or(ExpandErr::Syntax)?;
            if !is_char(cp) {
                return Err(ExpandErr::Syntax);
            }
            if mode == ExpandMode::Attr && (cp == 0x9 || cp == 0xA || cp == 0xD) {
                w.put(b' ')?;
            } else {
                w.put_slice(&src[i..i + bl])?;
            }
            i += bl;
            continue;
        }
        i += 1; /* past '&' */
        if src.get(i) == Some(&b'#') {
            i += 1;
            /* §4.1: the hex marker is a lowercase 'x' only. */
            let hex = src.get(i) == Some(&b'x');
            if hex {
                i += 1;
            }
            let base: u32 = if hex { 16 } else { 10 };
            let mut cp: u32 = 0;
            let mut ndigits = 0usize;
            loop {
                let d = match src.get(i) {
                    None | Some(&b';') => break,
                    Some(&d) => d,
                };
                let dig: u32 = match d {
                    b'0'..=b'9' => (d - b'0') as u32,
                    b'a'..=b'f' if hex => (d - b'a' + 10) as u32,
                    b'A'..=b'F' if hex => (d - b'A' + 10) as u32,
                    _ => return Err(ExpandErr::Syntax),
                };
                /* check BEFORE the multiply-add so a giant reference can never
                 * wrap into the valid range */
                if cp > (0x10FFFF - dig) / base {
                    return Err(ExpandErr::Syntax);
                }
                cp = cp * base + dig;
                ndigits += 1;
                i += 1;
            }
            if src.get(i) != Some(&b';') || ndigits == 0 {
                return Err(ExpandErr::Syntax);
            }
            i += 1; /* past ';' */
            if !is_char(cp) {
                return Err(ExpandErr::Syntax);
            }
            let mut enc = [0u8; 4];
            let n = utf8_encode(cp, &mut enc);
            w.put_slice(&enc[..n])?;
        } else {
            let nlen = src[i..]
                .iter()
                .position(|&b| b == b';')
                .ok_or(ExpandErr::Syntax)?;
            let name = &src[i..i + nlen];
            i += nlen + 1;
            let ch = match name {
                b"lt" => b'<',
                b"gt" => b'>',
                b"amp" => b'&',
                b"apos" => b'\'',
                b"quot" => b'"',
                _ => return Err(ExpandErr::Syntax),
            };
            w.put(ch)?;
        }
    }
    Ok(w.pos)
}

/// Fold CRLF and a lone CR to LF (§9.3b-A). Returns None when the input has
/// no CR (parse in place), else the normalized copy (which can only shrink).
/// The allocation is fallible so an OOM surfaces as a status, not an abort.
pub fn normalize_newlines(src: &[u8]) -> Result<Option<Vec<u8>>, ()> {
    if !src.contains(&b'\r') {
        return Ok(None);
    }
    let mut out: Vec<u8> = Vec::new();
    out.mkr_reserve_exact(src.len())?;
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
