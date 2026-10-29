//! Ruby encodings: a String's own, and transcoding one to UTF-8.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;

use crate::bridge::rb::{Error, RString, Value};
pub use crate::nupur::encoding::Encoding;

/* ---- encoding ---- */

/* Reading a String as UTF-8 is nupur's (`nupur::encoding`); these are its
 * functions under the names the bridge's readers use. */
pub(in crate::bridge) use crate::nupur::encoding::to_utf8_strict as utf8_strict;
pub use crate::nupur::encoding::{
    is_encoding_fault, to_encoding, to_utf8_replacing as ruby_to_utf8_value,
};

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
    Ok((e, super::ruby_string_bytes(ruby, name)?))
}

/// How a document parse reads `enc` (`html::encoding::transport_for`).
/// US-ASCII is read as binary, as XML's decode reads it
/// (`xml_decode::claims_no_encoding`): it is what `File.read` tags any bytes
/// with under `LANG=C`, not a claim. Only an encoding of a kind other than
/// binary, UTF-8 and US-ASCII is asked its name.
pub(in crate::bridge) fn encoding_kind(
    ruby: &crate::bridge::rb::Ruby,
    enc: Encoding,
) -> crate::html::encoding::StringEncoding<'static> {
    use crate::html::encoding::StringEncoding;
    if enc.is_ascii8bit() || enc.is_usascii() {
        StringEncoding::Binary
    } else if enc.is_utf8_compatible(ruby) {
        StringEncoding::Utf8
    } else {
        StringEncoding::Named(enc.name())
    }
}
