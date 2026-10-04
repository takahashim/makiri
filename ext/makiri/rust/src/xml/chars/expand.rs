//! Reference expansion: the five predefined entities and numeric character
//! references, written into a caller-supplied buffer.
//!
//! Its own module because it is an ENGINE, not a character class: it has a
//! cursor, a mode, a bounded writer and its own error domain, none of which the
//! classification half of [`super`] knows about. Its consumers are the XML
//! builder (`tree::Parser::expand`, which writes through it) and the DTD reader
//! (`tree::dtd`, which checks references with [`scan_reference`] and asks
//! [`only_unexpanded`] why an expansion failed) - one grammar for all three.

#![forbid(unsafe_code)]

use super::{is_char, validate_name, Utf8Char};
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

/// Where a walk's output goes: the caller's buffer, or nowhere when the walk
/// only checks the input ([`only_unexpanded`]).
trait Sink {
    fn put(&mut self, b: u8) -> Result<(), ExpandErr>;
    fn put_slice(&mut self, s: &[u8]) -> Result<(), ExpandErr>;
}

impl Sink for Writer<'_> {
    #[inline]
    fn put(&mut self, b: u8) -> Result<(), ExpandErr> {
        Writer::put(self, b)
    }
    #[inline]
    fn put_slice(&mut self, s: &[u8]) -> Result<(), ExpandErr> {
        Writer::put_slice(self, s)
    }
}

struct Discard;

impl Sink for Discard {
    #[inline]
    fn put(&mut self, _: u8) -> Result<(), ExpandErr> {
        Ok(())
    }
    #[inline]
    fn put_slice(&mut self, _: &[u8]) -> Result<(), ExpandErr> {
        Ok(())
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

/// One Reference (§4.1), as [`scan_reference`] reads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reference<'a> {
    /// A character reference, to a code point that is an XML `Char`.
    Char(u32),
    /// An entity reference, by a name that is an XML `Name`. Whether it can be
    /// expanded is the caller's question - the grammar does not say.
    Named(&'a [u8]),
}

/// One Reference (§4.1) after its `&`: what it refers to, and how many bytes
/// it occupied INCLUDING the closing ';'. `Err(Syntax)` for a malformed one -
/// a character reference without digits or to a code point XML has no `Char`
/// for (WFC: Legal Character), a name that is not a `Name`, no ';'. The one
/// reading of the grammar: the builder's expansion, the DTD's literals and the
/// classification of a failed expansion all go through it.
pub fn scan_reference(src: &[u8]) -> Result<(Reference<'_>, usize), ExpandErr> {
    if let Some(rest) = src.strip_prefix(b"#") {
        let (cp, used) = scan_char_ref(rest)?;
        return Ok((Reference::Char(cp), used + 1));
    }
    let nlen = src
        .iter()
        .position(|&b| b == b';')
        .ok_or(ExpandErr::Syntax)?;
    let name = &src[..nlen];
    if !validate_name(name) {
        return Err(ExpandErr::Syntax);
    }
    Ok((Reference::Named(name), nlen + 1))
}

/// What a named reference stands for, as the walk's caller rules.
enum Named {
    /// One of the five predefined entities: the byte it expands to.
    Byte(u8),
    /// An entity a DTD declares (or may, in an external subset), which Makiri
    /// does not expand.
    Unexpanded,
}

/// The one walk over character data: every character a `Char`, every '&' a
/// well-formed [`Reference`] - a character reference written out, a named one
/// as `named` rules (`None`: not a name it knows, a well-formedness error).
/// `Ok(true)` when some reference was [`Named::Unexpanded`].
fn walk<S: Sink>(
    src: &[u8],
    mode: ExpandMode,
    out: &mut S,
    mut named: impl FnMut(&[u8]) -> Option<Named>,
) -> Result<bool, ExpandErr> {
    let mut unexpanded = false;
    let mut i = 0usize;
    while i < src.len() {
        if src[i] != b'&' {
            let (cp, bl) = decode1(&src[i..]).ok_or(ExpandErr::Syntax)?;
            if !is_char(cp) {
                return Err(ExpandErr::Syntax);
            }
            if mode == ExpandMode::Attr && (cp == 0x9 || cp == 0xA || cp == 0xD) {
                out.put(b' ')?;
            } else {
                out.put_slice(&src[i..i + bl])?;
            }
            i += bl;
            continue;
        }
        let (reference, used) = scan_reference(&src[i + 1..])?;
        i += 1 + used;
        match reference {
            Reference::Char(cp) => out.put_slice(Utf8Char::encode(cp).as_bytes())?,
            Reference::Named(name) => match named(name).ok_or(ExpandErr::Syntax)? {
                Named::Byte(b) => out.put(b)?,
                Named::Unexpanded => unexpanded = true,
            },
        }
    }
    Ok(unexpanded)
}

/// Expand the 5 predefined entities + numeric character references in `src`
/// into `out` (which must hold at least `src.len()` bytes - the output is never
/// longer than the input), validating XML Char and, in Attr mode, folding
/// literal whitespace. Returns the number of bytes written.
pub fn expand_into(src: &[u8], mode: ExpandMode, out: &mut [u8]) -> Result<usize, ExpandErr> {
    let mut w = Writer { out, pos: 0 };
    walk(src, mode, &mut w, |name| {
        predefined_entity(name).map(Named::Byte)
    })?;
    Ok(w.pos)
}

/// Why [`expand_into`] refused `src`, when it did: `true` when the only thing
/// it could not expand is a reference to an entity `unexpanded` accepts - one
/// a DTD declares, which Makiri does not expand - and `false` when `src` is
/// malformed anyway: a character that is no `Char`, a malformed reference, or
/// a name nothing declares. A malformed input is never reported as merely
/// unexpanded.
pub fn only_unexpanded(src: &[u8], unexpanded: impl Fn(&[u8]) -> bool) -> bool {
    let named = |name: &[u8]| match predefined_entity(name) {
        Some(b) => Some(Named::Byte(b)),
        None => unexpanded(name).then_some(Named::Unexpanded),
    };
    walk(src, ExpandMode::Text, &mut Discard, named) == Ok(true)
}
