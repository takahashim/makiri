//! Reference expansion: the five predefined entities and numeric character
//! references, written into a caller-supplied buffer.
//!
//! Its own module because it is an ENGINE, not a character class: it has a
//! cursor, a mode, a bounded writer and its own error domain, none of which the
//! classification half of [`super`] knows about. `xml::arena`'s `Document::expand`
//! is the only consumer.

#![forbid(unsafe_code)]

use super::{is_char, Utf8Char};
use crate::cutf8::decode1;

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

/// One of the five entities XML predefines (§4.6). Every other name is a
/// reference to something a DTD would have to declare, which Makiri refuses
/// where it occurs - so this table is the whole set.
#[inline]
fn predefined_entity(name: &[u8]) -> Option<u8> {
    Some(match name {
        b"lt" => b'<',
        b"gt" => b'>',
        b"amp" => b'&',
        b"apos" => b'\'',
        b"quot" => b'"',
        _ => return None,
    })
}

/// A numeric character reference (§4.1) after its `&#`: the code point and how
/// many bytes it occupied, INCLUDING the closing ';'.
fn scan_char_ref(src: &[u8]) -> Result<(u32, usize), ExpandErr> {
    let mut i = 0usize;
    /* §4.1: the hex marker is a lowercase 'x' only. */
    let hex = src.first() == Some(&b'x');
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
    if !is_char(cp) {
        return Err(ExpandErr::Syntax);
    }
    Ok((cp, i + 1))
}

/// Expand the 5 predefined entities + numeric character references in `src`
/// into `out` (which must hold at least `src.len()` bytes - the output is never
/// longer than the input), validating XML Char and, in Attr mode, folding
/// literal whitespace. Returns the number of bytes written.
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
            let (cp, used) = scan_char_ref(&src[i..])?;
            i += used;
            w.put_slice(Utf8Char::encode(cp).as_bytes())?;
        } else {
            let nlen = src[i..]
                .iter()
                .position(|&b| b == b';')
                .ok_or(ExpandErr::Syntax)?;
            let name = &src[i..i + nlen];
            i += nlen + 1;
            w.put(predefined_entity(name).ok_or(ExpandErr::Syntax)?)?;
        }
    }
    Ok(w.pos)
}
