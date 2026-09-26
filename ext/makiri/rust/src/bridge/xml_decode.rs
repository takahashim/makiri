//! XML 1.0 Appendix F: choosing the input's encoding, and the strict decode to
//! UTF-8 the XML reader runs before it sees a byte.
//!
//! The byte reading - the BOM and the declaration's `encoding=` - is the
//! Ruby-free `xml::encoding_sniff`. This module adds the Ruby half: looking the
//! names up as `Encoding`s, weighing them against the String's own tag, and
//! transcoding.
//!
//! # The one ordering rule
//!
//! `rb_enc_find` can AUTOLOAD an encoding, which is a Ruby allocation and so a
//! GC point, and a borrow of the String's bytes must not be held across one.
//! So both scans finish while the bytes are borrowed, and the lookups run only
//! after that borrow has ended.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_long};

use crate::bridge::ruby::makiri_error;
use magnus::rb_sys::AsRawValue;
use magnus::{Error, RString};
use rb_sys::{rb_encoding, VALUE};

use super::ruby::exception_message;
use super::string::{ruby_bytes_view, text_check};
use crate::init::{EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::xml::encoding_sniff::sniff;

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

/// `rb_enc_find` for a name the sniffers produced; null for one Ruby does not
/// know, which is not an error here. May autoload an encoding - a GC point - so
/// it runs only once every borrow of the input is over.
///
/// `rb_enc_find` wants a C string and a sniffed name is bytes, so the NUL is
/// added HERE rather than in the sniffer, whose business is byte reading. A name
/// too long to fit, or holding a NUL, is not an encoding Ruby knows, so it reads
/// as "none" explicitly instead of being silently truncated at the NUL.
unsafe fn find_encoding(name: &[u8]) -> *mut rb_encoding {
    let mut buf = [0u8; 64];
    if name.is_empty() || name.len() >= buf.len() || name.contains(&0) {
        return core::ptr::null_mut();
    }
    buf[..name.len()].copy_from_slice(name);
    match core::ffi::CStr::from_bytes_with_nul(&buf[..name.len() + 1]) {
        Ok(c) => rb_sys::rb_enc_find(c.as_ptr()),
        Err(_) => core::ptr::null_mut(),
    }
}

/// Two encodings agree, for conflict purposes, when identical or when either is
/// US-ASCII (a subset of UTF-8 and of the single-byte encodings).
unsafe fn compatible(a: *mut rb_encoding, b: *mut rb_encoding) -> bool {
    a == b || a == rb_sys::rb_usascii_encoding() || b == rb_sys::rb_usascii_encoding()
}

/// Phase 1: the input's single effective byte encoding (XML 1.0 Appendix F).
///
/// A BOM wins, else the `<?xml encoding=?>` declaration, else the String's own
/// declared encoding - except ASCII-8BIT and US-ASCII, which claim no encoding
/// for the document and so are decoded by whatever was detected. Any
/// disagreement between the three is a fatal `Makiri::XML::SyntaxError`, so the
/// caller only ever sees one self-consistent answer.
///
/// US-ASCII is in that group because it is what a String gets with no claim
/// behind it - `File.read` under `LANG=C` - not a promise about the bytes: a
/// UTF-16 file with its BOM, or a Latin-1 one declaring ISO-8859-1, read that
/// way was validated as UTF-8 and refused, where the same bytes tagged
/// ASCII-8BIT decoded. With neither a BOM nor a declaration both still end up
/// validated as UTF-8, of which US-ASCII is a subset.
unsafe fn effective_encoding(str: RString) -> Result<*mut rb_encoding, Error> {
    let tag = rb_sys::rb_enc_get(str.as_raw());
    /* Read everything first, while the bytes are borrowed and nothing can run
     * a GC; the name lookups - which can - come after the borrow ends. */
    let (bom, decl) = {
        let anchor = ruby_bytes_view(str);
        let raw = anchor.bytes();
        sniff(raw)
    };
    let bom = bom.map_or(core::ptr::null_mut(), |b| {
        find_encoding(b.name().as_bytes())
    });
    let decl = decl.map_or(core::ptr::null_mut(), |d| find_encoding(d.as_bytes()));
    let is_binary = tag == rb_sys::rb_ascii8bit_encoding() || tag == rb_sys::rb_usascii_encoding();

    if !bom.is_null() && !decl.is_null() && !compatible(bom, decl) {
        return Err(syntax_error(
            "XML encoding conflict: the byte-order mark and the encoding declaration disagree",
        ));
    }
    if !is_binary && !bom.is_null() && !compatible(bom, tag) {
        return Err(syntax_error(
            "XML encoding conflict: the byte-order mark disagrees with the string's encoding",
        ));
    }
    if !is_binary && !decl.is_null() && !compatible(decl, tag) {
        /* A concrete String encoding is authoritative for decoding, so the
         * declaration is not used to transcode - but one naming a different
         * encoding than the String carries (a Shift_JIS String declaring
         * encoding="UTF-8") describes a self-inconsistent document, and that is
         * fatal rather than silently ignored. */
        return Err(syntax_error(
            "XML encoding conflict: the encoding declaration disagrees with the string's encoding",
        ));
    }

    if !is_binary {
        return Ok(tag);
    }
    if !bom.is_null() {
        return Ok(bom);
    }
    if !decl.is_null() {
        return Ok(decl);
    }
    Ok(rb_sys::rb_utf8_encoding())
}

/// A `Makiri::XML::SyntaxError` carrying `msg`.
fn syntax_error(msg: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(EXC_XML_SYNTAX_ERROR.exception(), msg)
}

/// A String the decode's own C calls returned, checked to be one before its
/// bytes are read - the same rule as a caller's value.
fn decoded_string(v: VALUE) -> Result<RString, Error> {
    // SAFETY: a live value the call just returned.
    RString::from_value(unsafe { crate::bridge::ruby::value(v) })
        .ok_or_else(|| makiri_error("XML input decoded to a non-String"))
}

/// Decode `str` to a validated, UTF-8-tagged, BOM-stripped String, or the
/// error that rejects it. A `max_bytes` of `None` skips the budget check (the
/// `__decode` test hook).
///
/// # Safety
/// Called with the GVL, as every bridge function is; `str` is a String by
/// type, and every String the decode makes is checked to be one.
unsafe fn xml_decode_input(str: RString, max_bytes: Option<usize>) -> Result<RString, Error> {
    let eff = effective_encoding(str)?;

    /* Phase 2: decode to UTF-8, strictly. UTF-8 / US-ASCII / ASCII-8BIT are
     * already UTF-8 bytes (validated below); anything else is transcoded in a
     * mode that raises instead of substituting U+FFFD. */
    let s = if eff == rb_sys::rb_utf8_encoding()
        || eff == rb_sys::rb_usascii_encoding()
        || eff == rb_sys::rb_ascii8bit_encoding()
    {
        str
    } else {
        let mut input = str.as_raw();
        if rb_sys::rb_enc_get(input) != eff {
            input = rb_sys::rb_str_dup(input);
            rb_sys::rb_enc_associate(input, eff);
        }
        let mut state: c_int = 0;
        let out = rb_sys::rb_protect(Some(strict_transcode_thunk), input, &mut state);
        if state != 0 {
            let exc = rb_sys::rb_errinfo();
            rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE);
            let msg = exception_message(exc);
            return Err(syntax_error(format!(
                "XML input could not be decoded to UTF-8: {msg}"
            )));
        }
        decoded_string(out)?
    };

    /* §4.3.3: a leading BOM is the encoding signature, not document content.
     * The transcode above turns any UTF-16/32 BOM into a U+FEFF, so one rule
     * covers every input. Each read of the bytes is a scoped borrow that ends
     * before anything can allocate - the module's one ordering rule. */
    let (off, len) = {
        let anchor = ruby_bytes_view(s);
        let bytes = anchor.bytes();
        let off = if bytes.starts_with(b"\xEF\xBB\xBF") {
            3
        } else {
            0
        };
        (off, bytes.len() - off)
    };

    /* Fail closed on an over-budget input BEFORE the validation scan and the
     * caller's GVL-release copy: an input whose UTF-8 length already exceeds the
     * arena budget can never parse. */
    if max_bytes.is_some_and(|max| len > max) {
        return Err(Error::new(
            EXC_XML_LIMIT_EXCEEDED.exception(),
            "XML input exceeds the byte budget",
        ));
    }

    /* Strict validation through the shared, allocation-free core. An embedded
     * NUL or any invalid UTF-8 is fatal; there is no U+FFFD repair here, unlike
     * the HTML sanitize path. The whole String is consulted for its cached
     * coderange (which covers the stripped suffix too - the BOM is one complete
     * UTF-8 character) while the bytes validated are the suffix. */
    let problem = {
        let anchor = ruby_bytes_view(s);
        text_check(s, &anchor.bytes()[off..]).problem()
    };
    if let Some(problem) = problem {
        return Err(syntax_error(format!("XML input {problem}")));
    }

    /* Built from the VALUE: the borrow above is over, and rb_str_subseq
     * allocates. */
    let u = rb_sys::rb_str_subseq(s.as_raw(), off as c_long, len as c_long);
    rb_sys::rb_enc_associate(u, rb_sys::rb_utf8_encoding());
    decoded_string(u)
}

/// [`xml_decode_input`] as a safe call: `s` is a String by type, and so is
/// the result.
pub fn xml_decode_input_value(s: RString, max_bytes: Option<usize>) -> Result<RString, Error> {
    // SAFETY: with the GVL, as every bridge function runs.
    unsafe { xml_decode_input(s, max_bytes) }
}
