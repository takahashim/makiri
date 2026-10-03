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
use rb_sys::VALUE;

use super::ruby::exception_message;
use super::string::{ruby_bytes_view, text_check, Encoding};
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

/// Two encodings agree, for conflict purposes, when identical or when either is
/// US-ASCII (a subset of UTF-8 and of the single-byte encodings).
fn compatible(a: Encoding, b: Encoding) -> bool {
    a == b || a.is_usascii() || b.is_usascii()
}

/// Whether a String's own tag claims no encoding for the document it holds:
/// ASCII-8BIT, and US-ASCII - an XML rule, not the HTML reader's. A String
/// tagged either is decoded by what its bytes declare (the BOM, then
/// `encoding=`), and validated as UTF-8 when they declare nothing.
///
/// US-ASCII is in that group because it is what a String gets with no claim
/// behind it - `File.read` under `LANG=C` - not a promise about the bytes: a
/// UTF-16 file with its BOM, or a Latin-1 one declaring ISO-8859-1, read that
/// way was validated as UTF-8 and refused, where the same bytes tagged
/// ASCII-8BIT decoded.
fn claims_no_encoding(tag: Encoding) -> bool {
    tag.is_ascii8bit() || tag.is_usascii()
}

/// Phase 1: the input's single effective byte encoding (XML 1.0 Appendix F).
///
/// A BOM wins, else the `<?xml encoding=?>` declaration, else the String's own
/// declared encoding - unless that claims none ([`claims_no_encoding`]), when
/// the input is decoded by whatever was detected. Any disagreement between the
/// three is a fatal `Makiri::XML::SyntaxError`, so the caller only ever sees one
/// self-consistent answer.
fn effective_encoding(str: RString) -> Result<Encoding, Error> {
    let tag = Encoding::of(str);
    /* Read everything first, while the bytes are borrowed and nothing can run
     * a GC; the name lookups - which can - come after the borrow ends. */
    let (bom, decl) = {
        // SAFETY: a live String, by type; the view, and the bytes borrowed
        // from it, are dropped before the lookups below can allocate.
        unsafe {
            let anchor = ruby_bytes_view(str);
            sniff(anchor.bytes())
        }
    };
    let bom = bom.and_then(|b| Encoding::find(b.name().as_bytes()));
    let decl = decl.and_then(|d| Encoding::find(d.as_bytes()));
    let claimed = !claims_no_encoding(tag);

    if let (Some(b), Some(d)) = (bom, decl) {
        if !compatible(b, d) {
            return Err(syntax_error(
                "XML encoding conflict: the byte-order mark and the encoding declaration disagree",
            ));
        }
    }
    if claimed && bom.is_some_and(|b| !compatible(b, tag)) {
        return Err(syntax_error(
            "XML encoding conflict: the byte-order mark disagrees with the string's encoding",
        ));
    }
    if claimed && decl.is_some_and(|d| !compatible(d, tag)) {
        /* A concrete String encoding is authoritative for decoding, so the
         * declaration is not used to transcode - but one naming a different
         * encoding than the String carries (a Shift_JIS String declaring
         * encoding="UTF-8") describes a self-inconsistent document, and that is
         * fatal rather than silently ignored. */
        return Err(syntax_error(
            "XML encoding conflict: the encoding declaration disagrees with the string's encoding",
        ));
    }

    if claimed {
        return Ok(tag);
    }
    Ok(bom.or(decl).unwrap_or_else(Encoding::utf8))
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

    /* Phase 2: decode to UTF-8, strictly. What is read as UTF-8 bytes is
     * validated below; anything else is transcoded in a mode that raises
     * instead of substituting U+FFFD. */
    let s = if eff.reads_as_utf8_bytes() {
        str
    } else {
        let mut input = str.as_raw();
        if Encoding::of(str) != eff {
            input = rb_sys::rb_str_dup(input);
            rb_sys::rb_enc_associate(input, eff.as_raw());
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
