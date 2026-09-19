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

use core::ffi::{c_char, c_int, c_long};

use magnus::rb_sys::AsRawValue;
use magnus::{Error, Value};
use rb_sys::{rb_encoding, VALUE};

use super::string::{ruby_bytes_view, ruby_exception_message, text_check};
use crate::init::{EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::xml::encoding_sniff::{sniff_bom, sniff_decl};

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
unsafe fn find_encoding(name: &core::ffi::CStr) -> *mut rb_encoding {
    rb_sys::rb_enc_find(name.as_ptr())
}

/// Two encodings agree, for conflict purposes, when identical or when either is
/// US-ASCII (a subset of UTF-8 and of the single-byte encodings).
unsafe fn compatible(a: *mut rb_encoding, b: *mut rb_encoding) -> bool {
    a == b || a == rb_sys::rb_usascii_encoding() || b == rb_sys::rb_usascii_encoding()
}

/// Phase 1: the input's single effective byte encoding (XML 1.0 Appendix F).
///
/// A BOM wins, else the `<?xml encoding=?>` declaration, else the String's own
/// declared encoding - except ASCII-8BIT, which means "raw bytes, no claimed
/// encoding" and so is decoded by whatever was detected. Any disagreement
/// between the three is a fatal `Makiri::XML::SyntaxError`, so the caller only
/// ever sees one self-consistent answer.
unsafe fn effective_encoding(str: VALUE) -> Result<*mut rb_encoding, Error> {
    let tag = rb_sys::rb_enc_get(str);
    /* Read everything first, while the bytes are borrowed and nothing can run
     * a GC; the name lookups - which can - come after the borrow ends. */
    let (bom, decl) = {
        let anchor = ruby_bytes_view(str);
        let raw = anchor.bytes();
        let (bom, geo) = sniff_bom(raw);
        (bom, sniff_decl(&raw[geo.bom_len.min(raw.len())..], geo))
    };
    let bom = bom.map_or(core::ptr::null_mut(), |b| find_encoding(b.name()));
    let decl = decl.map_or(core::ptr::null_mut(), |d| find_encoding(d.as_cstr()));
    let is_binary = tag == rb_sys::rb_ascii8bit_encoding();

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

/// Decode `str` to a validated, UTF-8-tagged, BOM-stripped String, or the
/// error that rejects it. `max_bytes` of 0 disables the budget check (the
/// `__decode` test hook).
pub unsafe fn xml_decode_input(str: VALUE, max_bytes: usize) -> Result<VALUE, Error> {
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
            ruby_exception_message(exc, msg.as_mut_ptr(), msg.len());
            let msg = core::ffi::CStr::from_ptr(msg.as_ptr()).to_string_lossy();
            return Err(syntax_error(format!(
                "XML input could not be decoded to UTF-8: {msg}"
            )));
        }
        out
    };

    let anchor = ruby_bytes_view(s);
    let bytes = anchor.bytes();
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
        return Err(Error::new(
            EXC_XML_LIMIT_EXCEEDED.exception(),
            "XML input exceeds the byte budget",
        ));
    }

    /* Strict validation through the shared, allocation-free core - no GC point
     * while the borrow is live. An embedded NUL or any invalid UTF-8 is fatal;
     * there is no U+FFFD repair here, unlike the HTML sanitize path. The whole
     * String is consulted for its cached coderange (which covers the stripped
     * suffix too - the BOM is one complete UTF-8 character) while the bytes
     * validated are the suffix. */
    if let Some(problem) = text_check(s, bytes.as_ptr().add(off) as *const c_char, len).problem() {
        return Err(syntax_error(format!("XML input {problem}")));
    }

    /* Build the result from the VALUE, not the borrow: rb_str_subseq allocates,
     * so the borrowed pointer must not be what it copies from. */
    let u = rb_sys::rb_str_subseq(s, off as c_long, len as c_long);
    rb_sys::rb_enc_associate(u, rb_sys::rb_utf8_encoding());
    Ok(u)
}

/// [`xml_decode_input`] as a safe call: `s` is a live String, and the result is
/// one.
pub fn xml_decode_input_value(s: Value, max_bytes: usize) -> Result<Value, Error> {
    // SAFETY: `s` is a live String; the decoder returns a live String.
    unsafe { xml_decode_input(s.as_raw(), max_bytes).map(|v| crate::bridge::ruby::value(v)) }
}
