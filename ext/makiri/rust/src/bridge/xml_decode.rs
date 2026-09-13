//! XML 1.0 Appendix F byte-encoding autodetection, and the strict decode to
//! UTF-8 the XML reader runs before it sees a byte (bridge/xml_decode.c).
//!
//! Split from [`super::string`] because it is an XML-specific charset
//! subsystem rather than a generic Ruby String helper. It borrows bytes only
//! through `mkr_ruby_bytes_view`, and shares that module's allocation-free
//! strict-text check.
//!
//! # Where the borrows are fragile
//!
//! `rb_enc_find` can AUTOLOAD an encoding, which is a Ruby allocation and so a
//! GC point. Two things follow, and both are load-bearing:
//!
//!   * the bytes are re-borrowed after the BOM lookup, because the view taken
//!     before it may no longer be valid;
//!   * the declaration scanner is kept allocation-free until every read is
//!     done - the interleave geometry is resolved by the BOM matcher and passed
//!     in rather than re-derived (which would need `rb_enc_find`), and the one
//!     name lookup happens after the bytes have been copied out.
//!
//! Where the C used `mkr_span` / `mkr_spanbuf` to make its reads bounded, this
//! uses slices, which are bounded by construction.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long};

use magnus::rb_sys::FromRawValue;
use magnus::{RString, Value};
use rb_sys::{rb_encoding, VALUE};

use super::string::{mkr_text_check, MKR_TEXT_HAS_NUL, MKR_TEXT_INVALID_UTF8};
use crate::glue::abi::{mkr_eXmlLimitExceeded, mkr_eXmlSyntaxError, rb_raise};

pub use crate::bridge::string::mkr_ruby_exception_message;

/// `rb_str_encode` with no replacement flags, so an undefined conversion or an
/// invalid byte sequence RAISES rather than substituting U+FFFD. Run under
/// `rb_protect` so the Ruby `Encoding::*Error` can be remapped to
/// `Makiri::XML::SyntaxError`.
unsafe extern "C" fn strict_transcode_thunk(str: VALUE) -> VALUE {
    rb_sys::rb_str_encode(
        str,
        rb_sys::rb_enc_from_encoding(rb_sys::rb_utf8_encoding()),
        0,
        rb_sys::Qnil as VALUE,
    )
}

/// The byte at `i`, or -1 past the end - the C's `mkr_span_at`, which lets the
/// scanner below read ahead without a bounds dance at every step.
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

/// How a detected encoding interleaves the ASCII column the declaration scanner
/// reads: `stride` bytes per character, the ASCII byte at `off`.
struct Geometry {
    bom_len: usize,
    stride: usize,
    off: usize,
}

/// The leading byte-order mark's encoding, or NULL, plus the geometry.
///
/// UTF-32 BOMs are tested before the UTF-16 LE BOM whose prefix they share. The
/// geometry is resolved here, at the match, rather than re-derived downstream:
/// that derivation needs `rb_enc_find`, and the scanner must stay
/// allocation-free while it holds a borrow.
unsafe fn bom_encoding(p: &[u8]) -> (*mut rb_encoding, Geometry) {
    let mut g = Geometry {
        bom_len: 0,
        stride: 1,
        off: 0,
    };
    let starts = |pat: &[u8]| p.starts_with(pat);
    if starts(b"\x00\x00\xFE\xFF") {
        g = Geometry {
            bom_len: 4,
            stride: 4,
            off: 3,
        };
        return (rb_sys::rb_enc_find(c"UTF-32BE".as_ptr()), g);
    }
    if starts(b"\xFF\xFE\x00\x00") {
        g = Geometry {
            bom_len: 4,
            stride: 4,
            off: 0,
        };
        return (rb_sys::rb_enc_find(c"UTF-32LE".as_ptr()), g);
    }
    if starts(b"\xFE\xFF") {
        g = Geometry {
            bom_len: 2,
            stride: 2,
            off: 1,
        };
        return (rb_sys::rb_enc_find(c"UTF-16BE".as_ptr()), g);
    }
    if starts(b"\xFF\xFE") {
        g = Geometry {
            bom_len: 2,
            stride: 2,
            off: 0,
        };
        return (rb_sys::rb_enc_find(c"UTF-16LE".as_ptr()), g);
    }
    if starts(b"\xEF\xBB\xBF") {
        g.bom_len = 3;
        return (rb_sys::rb_utf8_encoding(), g);
    }
    (core::ptr::null_mut(), g)
}

/// The encoding named in `<?xml ... encoding="NAME" ?>`, or NULL.
///
/// The declaration is ASCII, but for a UTF-16/32 document its bytes are
/// stride-interleaved, so the ASCII column is extracted first - which is what
/// lets a BOM-versus-declaration conflict be caught even in UTF-16.
unsafe fn decl_encoding(p: &[u8], stride: usize, off: usize) -> *mut rb_encoding {
    /* The extracted column, bounded by the buffer rather than by the loop
     * arithmetic. */
    let mut head = [0u8; 256];
    let mut hn = 0usize;
    let mut i = off;
    while hn < head.len() {
        let c = at(p, i);
        if c < 0 {
            break;
        }
        head[hn] = c as u8;
        hn += 1;
        i += stride;
    }
    let h = &head[..hn];

    let mut i = 0usize;
    while decl_ws(at(h, i)) {
        i += 1;
    }
    if !h[i.min(hn)..].starts_with(b"<?xml") {
        return core::ptr::null_mut();
    }
    i += 5;

    /* A whitespace-introduced "encoding" before the '?>'. */
    while i + 8 <= hn {
        if at(h, i) == b'?' as i32 && at(h, i + 1) == b'>' as i32 {
            return core::ptr::null_mut(); /* end of the declaration */
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
            return core::ptr::null_mut();
        }
        j += 1;
        while decl_ws(at(h, j)) {
            j += 1;
        }
        let q = at(h, j);
        if q != b'"' as i32 && q != b'\'' as i32 {
            return core::ptr::null_mut();
        }
        j += 1;
        let ns = j;
        while at(h, j) >= 0 && at(h, j) != q {
            j += 1;
        }
        if j >= hn {
            return core::ptr::null_mut();
        }
        let nl = j - ns;
        let mut name = [0u8; 64];
        if nl == 0 || nl >= name.len() {
            return core::ptr::null_mut();
        }
        name[..nl].copy_from_slice(&h[ns..j]);
        name[nl] = 0;
        /* Unknown names come back NULL, which is not an error here. */
        return rb_sys::rb_enc_find(name.as_ptr() as *const c_char);
    }
    core::ptr::null_mut()
}

/// Two encodings agree, for conflict purposes, when identical or when either is
/// US-ASCII (a subset of UTF-8 and of the single-byte encodings).
unsafe fn compatible(a: *mut rb_encoding, b: *mut rb_encoding) -> bool {
    a == b || a == rb_sys::rb_usascii_encoding() || b == rb_sys::rb_usascii_encoding()
}

/// The bytes of a String, borrowed. Valid only until Ruby runs.
unsafe fn bytes_of(str: VALUE) -> &'static [u8] {
    let r = RString::from_value(Value::from_raw(str)).expect("a T_STRING");
    let s = r.as_slice();
    core::slice::from_raw_parts(s.as_ptr(), s.len())
}

/// Phase 1: the input's single effective byte encoding (XML 1.0 Appendix F).
///
/// A BOM wins, else the `<?xml encoding=?>` declaration, else the String's own
/// declared encoding - except ASCII-8BIT, which means "raw bytes, no claimed
/// encoding" and so is decoded by whatever was detected. Any disagreement
/// between the three is a fatal `Makiri::XML::SyntaxError`, so the caller only
/// ever sees one self-consistent answer.
unsafe fn effective_encoding(str: VALUE) -> *mut rb_encoding {
    let tag = rb_sys::rb_enc_get(str);
    let (bom, geo) = bom_encoding(bytes_of(str));

    /* Re-borrow: the rb_enc_find inside the BOM lookup can autoload an encoding,
     * which is a GC point, and a borrow must not be held across one. */
    let raw = bytes_of(str);
    let decl = decl_encoding(&raw[geo.bom_len.min(raw.len())..], geo.stride, geo.off);
    let is_binary = tag == rb_sys::rb_ascii8bit_encoding();

    if !bom.is_null() && !decl.is_null() && !compatible(bom, decl) {
        rb_raise(
            mkr_eXmlSyntaxError,
            c"XML encoding conflict: the byte-order mark and the encoding declaration disagree"
                .as_ptr(),
        );
    }
    if !is_binary && !bom.is_null() && !compatible(bom, tag) {
        rb_raise(
            mkr_eXmlSyntaxError,
            c"XML encoding conflict: the byte-order mark disagrees with the string's encoding"
                .as_ptr(),
        );
    }
    if !is_binary && !decl.is_null() && !compatible(decl, tag) {
        /* A concrete String encoding is authoritative for decoding, so the
         * declaration is not used to transcode - but one naming a different
         * encoding than the String carries (a Shift_JIS String declaring
         * encoding="UTF-8") describes a self-inconsistent document, and that is
         * fatal rather than silently ignored. */
        rb_raise(
            mkr_eXmlSyntaxError,
            c"XML encoding conflict: the encoding declaration disagrees with the string's encoding"
                .as_ptr(),
        );
    }

    if !is_binary {
        return tag;
    }
    if !bom.is_null() {
        return bom;
    }
    if !decl.is_null() {
        return decl;
    }
    rb_sys::rb_utf8_encoding()
}

/// Decode `str` to a validated, UTF-8-tagged, BOM-stripped String, or raise.
/// `max_bytes` of 0 disables the budget check (the `__decode` test hook).
pub unsafe extern "C" fn mkr_xml_decode_input(str: VALUE, max_bytes: usize) -> VALUE {
    let eff = effective_encoding(str);

    /* Phase 2: decode to UTF-8, strictly. UTF-8 / US-ASCII / ASCII-8BIT are
     * already UTF-8 bytes (validated below); anything else is transcoded in a
     * mode that raises instead of substituting U+FFFD. */
    let s = if eff == rb_sys::rb_utf8_encoding()
        || eff == rb_sys::rb_usascii_encoding()
        || eff == rb_sys::rb_ascii8bit_encoding()
    {
        str
    } else {
        let mut input = str;
        if rb_sys::rb_enc_get(str) != eff {
            input = rb_sys::rb_str_dup(str);
            rb_sys::rb_enc_associate(input, eff);
        }
        let mut state: c_int = 0;
        let out = rb_sys::rb_protect(Some(strict_transcode_thunk), input, &mut state);
        if state != 0 {
            let exc = rb_sys::rb_errinfo();
            rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE);
            // c_char, not i8 - signed on aarch64-darwin, unsigned on
            // aarch64-linux (see the same fix in glue/xpath.rs).
            let mut msg = [0 as c_char; 256];
            mkr_ruby_exception_message(exc, msg.as_mut_ptr(), msg.len());
            rb_raise(
                mkr_eXmlSyntaxError,
                c"XML input could not be decoded to UTF-8: %s".as_ptr(),
                msg.as_ptr(),
            );
        }
        out
    };

    let bytes = bytes_of(s);
    /* §4.3.3: a leading BOM is the encoding signature, not document content.
     * The transcode above turns any UTF-16/32 BOM into a U+FEFF, so one rule
     * covers every input. */
    let off = if bytes.starts_with(b"\xEF\xBB\xBF") {
        3
    } else {
        0
    };
    let len = bytes.len() - off;

    /* Fail closed on an over-budget input BEFORE the validation scan and the
     * caller's GVL-release copy: an input whose UTF-8 length already exceeds the
     * arena budget can never parse. */
    if max_bytes != 0 && len > max_bytes {
        rb_raise(
            mkr_eXmlLimitExceeded,
            c"XML input exceeds the byte budget".as_ptr(),
        );
    }

    /* Strict validation through the shared, allocation-free core - no GC point
     * while the borrow is live. An embedded NUL or any invalid UTF-8 is fatal;
     * there is no U+FFFD repair here, unlike the HTML sanitize path. The whole
     * String is consulted for its cached coderange (which covers the stripped
     * suffix too - the BOM is one complete UTF-8 character) while the bytes
     * validated are the suffix. */
    match mkr_text_check(s, bytes.as_ptr().add(off) as *const c_char, len) {
        MKR_TEXT_HAS_NUL => rb_raise(
            mkr_eXmlSyntaxError,
            c"XML input must not contain a NUL byte".as_ptr(),
        ),
        MKR_TEXT_INVALID_UTF8 => rb_raise(
            mkr_eXmlSyntaxError,
            c"XML input must be valid UTF-8".as_ptr(),
        ),
        _ => {}
    }

    /* Build the result from the VALUE, not the borrow: rb_str_subseq allocates,
     * so the borrowed pointer must not be what it copies from. */
    let u = rb_sys::rb_str_subseq(s, off as c_long, len as c_long);
    rb_sys::rb_enc_associate(u, rb_sys::rb_utf8_encoding());
    u
}
