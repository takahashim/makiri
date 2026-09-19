//! XML 1.0 Appendix F: which encoding an XML document's first bytes announce -
//! a byte-order mark, and the `encoding="..."` of its XML declaration.
//!
//! Pure byte reading. Turning a name into a Ruby `Encoding`, and deciding
//! between this and the String's own tag, is the bridge's (`bridge::xml_decode`),
//! which runs its lookups only after these scans are done - so no GC point can
//! fall inside a borrow of the String's bytes.

use core::ffi::CStr;

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
    /// The encoding's name, as Ruby knows it.
    pub fn name(self) -> &'static CStr {
        match self {
            Bom::Utf32Be => c"UTF-32BE",
            Bom::Utf32Le => c"UTF-32LE",
            Bom::Utf16Be => c"UTF-16BE",
            Bom::Utf16Le => c"UTF-16LE",
            Bom::Utf8 => c"UTF-8",
        }
    }
}

/// How the detected encoding lays out the ASCII column the declaration
/// scanner reads: `stride` bytes per character, the ASCII byte at `off`, after
/// a `bom_len`-byte mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub bom_len: usize,
    pub stride: usize,
    pub off: usize,
}

/// The leading byte-order mark, if any, and the geometry it implies. UTF-32
/// marks are tested before the UTF-16 LE mark whose prefix they share.
pub fn sniff_bom(p: &[u8]) -> (Option<Bom>, Geometry) {
    let geo = |bom_len, stride, off| Geometry {
        bom_len,
        stride,
        off,
    };
    if p.starts_with(b"\x00\x00\xFE\xFF") {
        (Some(Bom::Utf32Be), geo(4, 4, 3))
    } else if p.starts_with(b"\xFF\xFE\x00\x00") {
        (Some(Bom::Utf32Le), geo(4, 4, 0))
    } else if p.starts_with(b"\xFE\xFF") {
        (Some(Bom::Utf16Be), geo(2, 2, 1))
    } else if p.starts_with(b"\xFF\xFE") {
        (Some(Bom::Utf16Le), geo(2, 2, 0))
    } else if p.starts_with(b"\xEF\xBB\xBF") {
        (Some(Bom::Utf8), geo(3, 1, 0))
    } else {
        (None, geo(0, 1, 0))
    }
}

/// An encoding name read from a declaration, NUL-terminated.
pub struct DeclName {
    buf: [u8; 64],
}

impl DeclName {
    pub fn as_cstr(&self) -> &CStr {
        /* The buffer is one longer than any name the scanner stores, so a NUL
         * always ends it; a NUL inside the name ends it there, as the C's
         * NUL-terminated lookup did. */
        CStr::from_bytes_until_nul(&self.buf).unwrap_or(c"")
    }
}

/// The byte at `i`, or -1 past the end.
#[inline]
fn at(s: &[u8], i: usize) -> i32 {
    match s.get(i) {
        Some(&b) => b as i32,
        None => -1,
    }
}

fn decl_ws(c: i32) -> bool {
    c == b' ' as i32 || c == b'\t' as i32 || c == b'\r' as i32 || c == b'\n' as i32
}

/// The encoding named in `<?xml ... encoding="NAME" ?>` at the start of `p`
/// (the bytes after any BOM), laid out by `geo`.
///
/// The declaration is ASCII, but in a UTF-16/32 document its bytes are
/// interleaved, so the ASCII column is extracted first - which is what lets a
/// BOM-versus-declaration conflict be caught even in UTF-16. `None` when there
/// is no declaration, it names no encoding, or the name is too long to be one.
pub fn sniff_decl(p: &[u8], geo: Geometry) -> Option<DeclName> {
    /* The extracted column, bounded by the buffer rather than by the loop
     * arithmetic. */
    let mut head = [0u8; 256];
    let mut hn = 0usize;
    let mut i = geo.off;
    while hn < head.len() {
        let c = at(p, i);
        if c < 0 {
            break;
        }
        head[hn] = c as u8;
        hn += 1;
        i += geo.stride;
    }
    let h = &head[..hn];

    let mut i = 0usize;
    while decl_ws(at(h, i)) {
        i += 1;
    }
    if !h[i.min(hn)..].starts_with(b"<?xml") {
        return None;
    }
    i += 5;

    /* A whitespace-introduced "encoding" before the '?>'. */
    while i + 8 <= hn {
        if at(h, i) == b'?' as i32 && at(h, i + 1) == b'>' as i32 {
            return None; /* end of the declaration */
        }
        if !decl_ws(at(h, i.wrapping_sub(1))) || !h[i..].starts_with(b"encoding") {
            i += 1;
            continue;
        }
        let mut j = i + 8;
        while decl_ws(at(h, j)) {
            j += 1;
        }
        if at(h, j) != b'=' as i32 {
            return None;
        }
        j += 1;
        while decl_ws(at(h, j)) {
            j += 1;
        }
        let q = at(h, j);
        if q != b'"' as i32 && q != b'\'' as i32 {
            return None;
        }
        j += 1;
        let ns = j;
        while at(h, j) >= 0 && at(h, j) != q {
            j += 1;
        }
        if j >= hn {
            return None;
        }
        let nl = j - ns;
        let mut name = DeclName { buf: [0u8; 64] };
        if nl == 0 || nl >= name.buf.len() {
            return None;
        }
        name.buf[..nl].copy_from_slice(&h[ns..j]);
        return Some(name);
    }
    None
}
