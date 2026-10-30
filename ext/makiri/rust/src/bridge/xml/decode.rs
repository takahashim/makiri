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
//! So nothing here borrows the bytes: the sniff reads a copy of the
//! document's first bytes (`string::prefix`, `SNIFF_WINDOW` of them, all a
//! sniff reads), and the decoded document is copied once, BOM stripped, into
//! the buffer the parser reads - which is what the strict check reads too.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, RString};

use crate::bridge::string::{
    claims_no_encoding, prefix, ruby_string_bytes_from, text_check, Encoding,
};
use crate::cbuf::OwnedBuf;
use crate::init::{EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::xml::encoding_sniff::{sniff, SNIFF_WINDOW};

/// Two encodings agree, for conflict purposes, when identical or when either is
/// US-ASCII (a subset of UTF-8 and of the single-byte encodings).
fn compatible(a: Encoding, b: Encoding) -> bool {
    a == b || a.is_usascii() || b.is_usascii()
}

/// Phase 1: the input's single effective byte encoding (XML 1.0 Appendix F).
///
/// A BOM wins, else the `<?xml encoding=?>` declaration, else the String's own
/// declared encoding - unless that claims none ([`claims_no_encoding`]), when
/// the input is decoded by whatever was detected. Any disagreement between the
/// three is a fatal `Makiri::XML::SyntaxError`, so the caller only ever sees one
/// self-consistent answer. With it, whether the declaration names US-ASCII,
/// which every other encoding is taken to agree with ([`compatible`]) but
/// which holds the document to ASCII whatever it is decoded by.
fn effective_encoding(ruby: &Ruby, str: RString) -> Result<(Encoding, bool), Error> {
    let tag = Encoding::of(str);
    /* The sniff reads a copy of the first bytes; the name lookups - which can
     * allocate - come after. */
    let (head, n) = prefix::<SNIFF_WINDOW>(str);
    let (bom, decl) = sniff(head.get(..n).unwrap_or(&[]));
    let bom = match bom {
        Some(b) => Encoding::find(ruby, b.name().as_bytes())?,
        None => None,
    };
    /* XML 1.0 §4.3.3: the name is an IANA charset name or alias, Ruby's or
     * one `encoding_names` maps to Ruby's; an entity in an encoding the
     * processor cannot process is a fatal error - never read as another. */
    let decl = match decl {
        None => None,
        Some(d) => Some(declared_encoding(ruby, d.as_bytes())?.ok_or_else(|| {
            syntax_error(format_args!(
                "XML encoding declaration names an unknown encoding: {}",
                crate::msg::Lossy(d.as_bytes())
            ))
        })?),
    };
    let claimed = !claims_no_encoding(tag);
    let declares_ascii = decl.is_some_and(|d| d.is_usascii());

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
        return Ok((tag, declares_ascii));
    }
    Ok((
        bom.or(decl).unwrap_or_else(|| Encoding::utf8(ruby)),
        declares_ascii,
    ))
}

/// The encoding an encoding declaration names: Ruby's for an IANA name or
/// alias (`encoding_names` supplying the aliases Ruby lacks). Ruby's own
/// names for the process's encodings (`locale`, `external`, ...) name no
/// charset, so a document cannot reach them. An `Err` is
/// [`Encoding::find`]'s, passed on.
fn declared_encoding(ruby: &Ruby, name: &[u8]) -> Result<Option<Encoding>, Error> {
    if crate::xml::encoding_names::is_ruby_special(name) {
        return Ok(None);
    }
    let found = match Encoding::find(ruby, name)? {
        Some(e) => Some(e),
        None => match crate::xml::encoding_names::ruby_name(name) {
            Some(n) => Encoding::find(ruby, n)?,
            None => None,
        },
    };
    /* ASCII-8BIT is Ruby's "no encoding" (`BINARY`, `ASCII-8BIT`), not an
     * IANA charset: a document naming it names nothing XML can decode by,
     * and would otherwise be read as UTF-8. */
    Ok(found.filter(|e| !e.is_ascii8bit()))
}

/// A `Makiri::XML::SyntaxError` carrying `msg`.
fn syntax_error(msg: impl crate::bridge::rb::Message) -> Error {
    Error::new(&EXC_XML_SYNTAX_ERROR, msg)
}

/// Phase 2: `str` as UTF-8 text. What is read as UTF-8 bytes is `str` itself
/// (validated by the caller); anything else is transcoded in a mode that
/// raises instead of substituting U+FFFD, so an undefined conversion or an
/// invalid byte sequence is an `Encoding::*Error` - remapped to
/// `Makiri::XML::SyntaxError`. Any other failure (`NoMemoryError`, an
/// interrupt) is passed through as it is.
fn strict_utf8(ruby: &Ruby, str: RString, eff: Encoding) -> Result<RString, Error> {
    use crate::nupur::encoding::Unconvertible;
    use crate::nupur::string::Transcode;
    match crate::nupur::string::to_utf8(ruby, str, Some(eff), Unconvertible::Raise)? {
        Transcode::AsIs(s) | Transcode::Transcoded(s) => Ok(s),
        Transcode::Fault(e) => Err(syntax_error(format_args!(
            "XML input could not be decoded to UTF-8: {}",
            e.wording(ruby)
        ))),
    }
}

/// How many bytes of a leading BOM `s` (UTF-8 by now) starts with, 0 for
/// none. §4.3.3: a BOM is the encoding signature, not document content; the
/// transcode turns any UTF-16/32 BOM into a U+FEFF, so one rule covers every
/// input.
fn bom_len(s: RString) -> usize {
    let (head, n) = prefix::<3>(s);
    if n == 3 && head == *b"\xEF\xBB\xBF" {
        3
    } else {
        0
    }
}

/// Decode `str` to validated, BOM-stripped UTF-8, copied into the buffer the
/// parser reads, or the error that rejects it. A `max_bytes` of `None` skips
/// the budget check (the `__decode` test hook).
pub fn xml_decode_input(
    ruby: &Ruby,
    str: RString,
    max_bytes: Option<usize>,
) -> Result<OwnedBuf, Error> {
    let (eff, declares_ascii) = effective_encoding(ruby, str)?;
    let s = strict_utf8(ruby, str, eff)?;
    /* US-ASCII is read as UTF-8 bytes, so it needs no transcode, and every
     * encoding is taken to agree with it - but a document DECLARING US-ASCII
     * holds ASCII alone (§4.3.3: bytes the declared encoding cannot read are
     * a fatal error), whatever it is decoded by: the String's own encoding
     * (a UTF-8 String, the common one), a BOM, or the declaration itself. A
     * String's own US-ASCII is no declaration: it claims none. */
    let ascii_only = declares_ascii;
    let off = bom_len(s);
    let len = s.len() - off;

    /* Fail closed on an over-budget input BEFORE the copy and the validation
     * scan: an input whose UTF-8 length already exceeds the arena budget can
     * never parse. */
    if max_bytes.is_some_and(|max| len > max) {
        return Err(Error::new(
            &EXC_XML_LIMIT_EXCEEDED,
            "XML input exceeds the byte budget",
        ));
    }

    /* The one copy: the bytes the parser reads, with the GVL released for a
     * document. Strict validation is of that copy, through the shared,
     * allocation-free core. An embedded NUL or any invalid UTF-8 is fatal;
     * there is no U+FFFD repair here, unlike the HTML sanitize path. The
     * whole String is consulted for its cached coderange (which covers the
     * stripped prefix too - the BOM is one complete UTF-8 character). */
    let bytes = ruby_string_bytes_from(s, off)?;
    if let Some(problem) = text_check(s, bytes.as_slice()).problem() {
        return Err(syntax_error(format_args!("XML input {problem}")));
    }
    if ascii_only && !bytes.as_slice().is_ascii() {
        return Err(syntax_error(
            "XML input declared US-ASCII holds a byte outside ASCII",
        ));
    }
    Ok(bytes)
}
