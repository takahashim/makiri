//! Ruby encodings: a String's own, and transcoding one to UTF-8.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;

use crate::bridge::rb::{Error, RString, Value};
pub use crate::nupur::encoding::Encoding;

/* ---- encoding ---- */

/* Reading a String as UTF-8 is nupur's (`nupur::encoding`). */
pub use crate::nupur::encoding::to_encoding;

/// `v` as an encoding argument: the encoding, and the name to write for it -
/// the String or Symbol as the caller wrote it, or an `Encoding`'s own name.
pub fn encoding_arg(ruby: &Ruby, v: Value) -> Result<(Encoding, crate::cbuf::OwnedBuf), Error> {
    let e = to_encoding(ruby, v)?;
    let name = match RString::from_value(v) {
        Some(s) => s,
        None if crate::bridge::rb::Symbol::from_value(v).is_some() => {
            crate::bridge::ruby::name_arg(v)?
        }
        None => {
            return crate::cbuf::OwnedBuf::copy_from(e.name())
                .map(|b| (e, b))
                .ok_or_else(|| {
                    crate::bridge::ruby::makiri_error("out of memory reading an encoding name")
                });
        }
    };
    Ok((e, super::ruby_string_bytes(name)?))
}

/// Whether a String's own tag claims no encoding for the document it holds:
/// ASCII-8BIT, and US-ASCII. A document parse - HTML's and XML's alike -
/// reads such a String by what its bytes declare (a BOM, a `<meta>` or
/// `encoding=`), and as UTF-8 when they declare nothing.
///
/// US-ASCII is in that group because it is what a String gets with no claim
/// behind it - `File.read` under `LANG=C` - not a promise about the bytes: a
/// UTF-16 file with its BOM, or a Latin-1 one declaring ISO-8859-1, read that
/// way was validated as UTF-8 and refused, where the same bytes tagged
/// ASCII-8BIT decoded. (A US-ASCII a DOCUMENT declares is a claim:
/// `xml_decode` holds the bytes to it.)
pub(in crate::bridge) fn claims_no_encoding(tag: Encoding) -> bool {
    tag.is_ascii8bit() || tag.is_usascii()
}

/// How a document parse reads `enc` (`html::encoding::transport_for`): a
/// String that claims no encoding ([`claims_no_encoding`]) as binary. Only
/// an encoding of a kind other than binary, UTF-8 and US-ASCII is asked its
/// name.
pub(in crate::bridge) fn encoding_kind(
    ruby: &crate::bridge::rb::Ruby,
    enc: Encoding,
) -> crate::html::encoding::StringEncoding<'static> {
    use crate::html::encoding::StringEncoding;
    if claims_no_encoding(enc) {
        StringEncoding::Binary
    } else if enc.is_utf8_or_ascii(ruby) {
        StringEncoding::Utf8
    } else {
        StringEncoding::Named(enc.name())
    }
}
