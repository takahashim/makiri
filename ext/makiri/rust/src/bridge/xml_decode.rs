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

use super::string::{prefix, ruby_string_bytes_from, text_check, Encoding};
use crate::cbuf::OwnedBuf;
use crate::init::{EXC_XML_LIMIT_EXCEEDED, EXC_XML_SYNTAX_ERROR};
use crate::xml::encoding_sniff::{sniff, SNIFF_WINDOW};

/// Two encodings agree, for conflict purposes, when identical or when either is
/// US-ASCII (a subset of UTF-8 and of the single-byte encodings).
fn compatible(a: Encoding, b: Encoding) -> bool {
    a == b || a.is_usascii() || b.is_usascii()
}

/// Whether a String's own tag claims no encoding for the document it holds:
/// ASCII-8BIT, and US-ASCII - as the HTML document reader has it
/// (`Encoding::kind`). A String
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
fn effective_encoding(ruby: &Ruby, str: RString) -> Result<Encoding, Error> {
    let tag = Encoding::of(str);
    /* The sniff reads a copy of the first bytes; the name lookups - which can
     * allocate - come after. */
    let (head, n) = prefix::<SNIFF_WINDOW>(ruby, str);
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
    Ok(bom.or(decl).unwrap_or_else(|| Encoding::utf8(ruby)))
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
    if let Some(e) = Encoding::find(ruby, name)? {
        return Ok(Some(e));
    }
    match crate::xml::encoding_names::ruby_name(name) {
        Some(n) => Encoding::find(ruby, n),
        None => Ok(None),
    }
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
    if eff.reads_as_utf8_bytes(ruby) {
        return Ok(str);
    }
    let mut input = str;
    if Encoding::of(str) != eff {
        input = super::raw::str_dup(input)?;
        super::raw::enc_associate(input, eff)?;
    }
    super::raw::str_encode(input, Encoding::utf8(ruby), 0).or_else(|e| {
        /* Worded first, as the message is the raise's own; the error is then
         * handled only if it is an `EncodingError`, and passed on otherwise. */
        let refusal = syntax_error(format_args!(
            "XML input could not be decoded to UTF-8: {}",
            e.wording(ruby)
        ));
        e.rescue(ruby, &[crate::bridge::rb::Builtin::EncodingError])?;
        Err(refusal)
    })
}

/// How many bytes of a leading BOM `s` (UTF-8 by now) starts with, 0 for
/// none. §4.3.3: a BOM is the encoding signature, not document content; the
/// transcode turns any UTF-16/32 BOM into a U+FEFF, so one rule covers every
/// input.
fn bom_len(ruby: &Ruby, s: RString) -> usize {
    let (head, n) = prefix::<3>(ruby, s);
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
    let eff = effective_encoding(ruby, str)?;
    let s = strict_utf8(ruby, str, eff)?;
    let off = bom_len(ruby, s);
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
    let bytes = ruby_string_bytes_from(ruby, s, off)?;
    if let Some(problem) = text_check(ruby, s, bytes.as_slice()).problem() {
        return Err(syntax_error(format_args!("XML input {problem}")));
    }
    Ok(bytes)
}
