//! XML 1.0 Appendix F: which encoding an XML document's first bytes announce -
//! a byte-order mark, and the `encoding="..."` of its XML declaration.
//!
//! Pure byte reading, and byte reading only: [`sniff`] answers with the mark and
//! the declared NAME. Turning a name into a Ruby `Encoding`, and deciding
//! between those and the String's own tag, is the bridge's
//! (`bridge::xml_decode`), which runs its lookups only after these scans are
//! done - so no GC point can fall inside a borrow of the String's bytes. Nothing
//! here is shaped by how that lookup wants its argument.

#![forbid(unsafe_code)]

/// A byte-order mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bom {
    Utf32Be,
    Utf32Le,
    Utf16Be,
    Utf16Le,
    Utf8,
}

impl Bom {
    /// The encoding's name.
    pub fn name(self) -> &'static str {
        match self {
            Bom::Utf32Be => "UTF-32BE",
            Bom::Utf32Le => "UTF-32LE",
            Bom::Utf16Be => "UTF-16BE",
            Bom::Utf16Le => "UTF-16LE",
            Bom::Utf8 => "UTF-8",
        }
    }
}

/// An encoding name read from a declaration.
///
/// Owned inline: it is scanned out of a borrow of the input that must end before
/// the bridge may look the name up. The longest encoding name Ruby knows is far
/// shorter than this, so a name that does not fit is not one - [`DeclName::new`]
/// refuses it rather than truncating.
pub struct DeclName {
    buf: [u8; 63],
    len: usize,
}

impl DeclName {
    fn new(name: &[u8]) -> Option<DeclName> {
        let mut buf = [0u8; 63];
        if name.is_empty() || name.len() > buf.len() {
            return None;
        }
        buf[..name.len()].copy_from_slice(name);
        Some(DeclName {
            buf,
            len: name.len(),
        })
    }

    /// Exactly the bytes between the quotes. Whether they name an encoding this
    /// runtime knows is the caller's question.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// How the detected encoding lays out the ASCII column the declaration scanner
/// reads: `stride` bytes per character, the ASCII byte at `off`, after a
/// `bom_len`-byte mark.
///
/// Private: it describes an internal layout, and handing it to the caller only
/// to have the caller hand it back - after stripping `bom_len` itself - split
/// one decision across the module boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    bom_len: usize,
    stride: usize,
    off: usize,
}

/// The marks and the geometry each implies.
///
/// Row ORDER is the rule: the UTF-32 marks must be tested before the UTF-16 LE
/// mark whose prefix they share (`FF FE` opens `FF FE 00 00`), which is checkable
/// by eye here rather than by following a chain of `else if`s.
const BOMS: &[(&[u8], Bom, Geometry)] = &[
    (
        b"\x00\x00\xFE\xFF",
        Bom::Utf32Be,
        Geometry {
            bom_len: 4,
            stride: 4,
            off: 3,
        },
    ),
    (
        b"\xFF\xFE\x00\x00",
        Bom::Utf32Le,
        Geometry {
            bom_len: 4,
            stride: 4,
            off: 0,
        },
    ),
    (
        b"\xFE\xFF",
        Bom::Utf16Be,
        Geometry {
            bom_len: 2,
            stride: 2,
            off: 1,
        },
    ),
    (
        b"\xFF\xFE",
        Bom::Utf16Le,
        Geometry {
            bom_len: 2,
            stride: 2,
            off: 0,
        },
    ),
    (
        b"\xEF\xBB\xBF",
        Bom::Utf8,
        Geometry {
            bom_len: 3,
            stride: 1,
            off: 0,
        },
    ),
];

const NO_BOM: Geometry = Geometry {
    bom_len: 0,
    stride: 1,
    off: 0,
};

/// What `p`'s first bytes announce: the byte-order mark, and the encoding its
/// XML declaration names.
///
/// One entry point, because the two are one decision: the mark fixes the
/// geometry the declaration has to be read through, and a conflict between them
/// is only detectable when both come from the same reading.
pub fn sniff(p: &[u8]) -> (Option<Bom>, Option<DeclName>) {
    let (bom, geo) = sniff_bom(p);
    let body = &p[geo.bom_len.min(p.len())..];
    (bom, sniff_decl(body, geo))
}

fn sniff_bom(p: &[u8]) -> (Option<Bom>, Geometry) {
    for &(mark, bom, geo) in BOMS {
        if p.starts_with(mark) {
            return (Some(bom), geo);
        }
    }
    (None, NO_BOM)
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// The ASCII column of the document's first bytes, with a cursor over it.
///
/// A UTF-16/32 document's XML declaration is still ASCII, but its bytes are
/// interleaved, so the column takes one byte every `stride`. Extracting it first
/// is what lets a BOM-versus-declaration conflict be caught even in UTF-16, and
/// it makes the scan below ordinary ASCII scanning over `Option<u8>` - where the
/// C used `-1` for BOTH "past the end" and "before the start", so the safety of
/// reading index `i - 1` at `i == 0` rested on two unrelated functions happening
/// to agree.
struct Column {
    buf: [u8; 256],
    len: usize,
    i: usize,
}

impl Column {
    /// The column `geo` describes, bounded by the buffer rather than by the loop
    /// arithmetic.
    fn extract(p: &[u8], geo: Geometry) -> Column {
        let mut c = Column {
            buf: [0u8; 256],
            len: 0,
            i: 0,
        };
        let mut at = geo.off;
        while c.len < c.buf.len() {
            let Some(&b) = p.get(at) else { break };
            c.buf[c.len] = b;
            c.len += 1;
            at += geo.stride;
        }
        c
    }

    #[inline]
    fn rest(&self) -> &[u8] {
        &self.buf[self.i.min(self.len)..self.len]
    }
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.rest().first().copied()
    }
    /// The byte BEFORE the cursor; `None` at the very start, which is not
    /// whitespace.
    #[inline]
    fn prev(&self) -> Option<u8> {
        self.buf[..self.len].get(self.i.checked_sub(1)?).copied()
    }
    #[inline]
    fn starts(&self, lit: &[u8]) -> bool {
        self.rest().starts_with(lit)
    }
    #[inline]
    fn advance(&mut self, n: usize) {
        self.i = self.i.saturating_add(n).min(self.len);
    }
    fn eat(&mut self, lit: &[u8]) -> bool {
        if !self.starts(lit) {
            return false;
        }
        self.advance(lit.len());
        true
    }
    fn skip_ws(&mut self) {
        while self.peek().is_some_and(is_ws) {
            self.advance(1);
        }
    }
    /// The bytes up to the next `q`, consuming it. `None` when the column ends
    /// first, so an unterminated literal is not read as a name.
    fn take_until(&mut self, q: u8) -> Option<&[u8]> {
        let n = self.rest().iter().position(|&b| b == q)?;
        let from = self.i;
        self.advance(n + 1);
        Some(&self.buf[from..from + n])
    }
}

/// The encoding named in `<?xml ... encoding="NAME" ?>` at the start of `p` (the
/// bytes after any BOM), laid out by `geo`. `None` when there is no declaration,
/// it names no encoding, or the name is too long to be one.
fn sniff_decl(p: &[u8], geo: Geometry) -> Option<DeclName> {
    let mut c = Column::extract(p, geo);
    c.skip_ws();
    if !c.eat(b"<?xml") {
        return None;
    }
    /* A whitespace-introduced "encoding", before the '?>' that ends the
     * declaration. The preceding byte must be space so `standalone-encoding`
     * and the like cannot match. */
    loop {
        if c.rest().len() < b"encoding".len() {
            return None;
        }
        if c.starts(b"?>") {
            return None;
        }
        if c.prev().is_some_and(is_ws) && c.starts(b"encoding") {
            c.advance(b"encoding".len());
            break;
        }
        c.advance(1);
    }
    c.skip_ws();
    if c.peek() != Some(b'=') {
        return None;
    }
    c.advance(1);
    c.skip_ws();
    let q = match c.peek() {
        Some(q @ (b'"' | b'\'')) => q,
        _ => return None,
    };
    c.advance(1);
    DeclName::new(c.take_until(q)?)
}
