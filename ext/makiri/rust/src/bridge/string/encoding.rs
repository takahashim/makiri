//! Ruby encodings: a String's own, and transcoding one to UTF-8.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use crate::bridge::rb::value::AsRawValue;
use crate::bridge::rb::{prelude::*, Error, RString, Value};
pub use crate::nupur::encoding::Encoding;

/* ---- encoding ---- */

/// `v` as an encoding argument: the encoding, and the name to write for it -
/// the String or Symbol as the caller wrote it, or an `Encoding`'s own name.
pub fn encoding_arg(v: Value) -> Result<(Encoding, crate::cbuf::OwnedBuf), Error> {
    let e = to_encoding(v)?;
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

impl Encoding {
    /// How a document parse reads this encoding
    /// (`html::encoding::transport_for`). US-ASCII is read as binary, as
    /// XML's decode reads it (`xml_decode::claims_no_encoding`): it is what
    /// `File.read` tags any bytes with under `LANG=C`, not a claim. Only an encoding of a kind other
    /// than binary, UTF-8 and US-ASCII is asked its name.
    pub(in crate::bridge) fn kind(self) -> crate::html::encoding::StringEncoding<'static> {
        use crate::html::encoding::StringEncoding;
        if self.is_ascii8bit() || self.is_usascii() {
            StringEncoding::Binary
        } else if self.is_utf8_compatible() {
            StringEncoding::Utf8
        } else {
            StringEncoding::Named(self.name())
        }
    }
}

/// The encoding `v` names, or the error Ruby's own lookup raises: `ArgumentError`
/// for an unknown name. `v` is an `Encoding`, a String or a Symbol, and
/// anything else is a `TypeError` before the lookup, which would run its
/// `to_str`.
pub fn to_encoding(v: Value) -> Result<Encoding, Error> {
    let v = if v.is_kind_of(crate::bridge::ruby::gvl_ruby().class_encoding()) {
        v
    } else {
        crate::bridge::ruby::name_arg(v)?.as_value()
    };
    crate::bridge::raw::to_encoding(v)
}

/// [`Encoding::reads_as_utf8_bytes`] for `s`'s encoding, by index and with
/// no call on the common path: the index is read from the String's flags,
/// where Ruby keeps it inline for every encoding below
/// `RUBY_ENCODING_INLINE_MAX` (the builtin three are 0-2), and compared with
/// the builtin indices, looked up once. Every strict read (a selector, an
/// attribute name or value) asks, and `rb_enc_get` + `rb_utf8_encoding`
/// cost `Node#[]` ~13%.
fn reads_as_utf8_bytes(s: RString) -> bool {
    use rb_sys::ruby_encoding_consts::{
        RUBY_ENCODING_INLINE_MAX, RUBY_ENCODING_MASK, RUBY_ENCODING_SHIFT,
    };
    static BUILTIN: std::sync::OnceLock<[c_int; 3]> = std::sync::OnceLock::new();
    // SAFETY: `s` is a live String - a heap object, whose flags word is the
    // first field of its `RBasic` and readable; reading it runs no Ruby.
    let flags = unsafe { (*(s.as_raw() as *const rb_sys::RBasic)).flags };
    let inline =
        ((flags as usize & RUBY_ENCODING_MASK as usize) >> RUBY_ENCODING_SHIFT as u32) as c_int;
    let index = if inline == RUBY_ENCODING_INLINE_MAX as c_int {
        // SAFETY: as above; the lookup reads an instance variable and raises
        // nothing.
        unsafe { rb_sys::rb_enc_get_index(s.as_raw()) }
    } else {
        inline
    };
    // SAFETY: Ruby's immutable builtin encoding indices.
    let builtin = BUILTIN.get_or_init(|| unsafe {
        [
            rb_sys::rb_utf8_encindex(),
            rb_sys::rb_usascii_encindex(),
            rb_sys::rb_ascii8bit_encindex(),
        ]
    });
    builtin.contains(&index)
}

/// `s` as a UTF-8 String, transcoded by Ruby with `ecflags` unless its bytes
/// are read as UTF-8 already ([`Encoding::reads_as_utf8_bytes`] - UTF-8,
/// US-ASCII, ASCII-8BIT), which costs one encoding comparison: no transcode,
/// no copy, no `protect`.
///
/// `rb_str_encode` RAISES - when Ruby has no converter at all (UTF-7,
/// ISO-2022-JP-2 to UTF-8), and with no replacement flag on any byte or
/// character it cannot convert - and the raise comes back as `Err`.
fn ruby_to_utf8(s: RString, ecflags: c_int) -> Result<RString, Error> {
    if reads_as_utf8_bytes(s) {
        return Ok(s);
    }
    crate::bridge::raw::str_encode(s, Encoding::utf8(), ecflags)
}

/// `s` as UTF-8 for a STRICT reader (`RubyText`, `RubyData`): transcoded with
/// no replacement, so a character that has no UTF-8 form, or a byte that is
/// not one of the encoding's, is an error rather than a U+FFFD. The parse
/// path replaces instead ([`ruby_to_utf8_value`]); a selector or an attribute
/// value is not repaired. Ruby's own raise (an `Encoding::*Error`, or
/// whatever else interrupted the transcode) is returned.
pub(in crate::bridge) fn utf8_strict(s: RString) -> Result<RString, Error> {
    ruby_to_utf8(s, 0)
}

/// A UTF-8 String for `s`, honouring its declared encoding so the content
/// survives.
///
///  - UTF-8 / US-ASCII / ASCII-8BIT: returned unchanged. These are already
///    UTF-8 bytes, or deliberately raw ones, and the native parser does the
///    WHATWG invalid-byte replacement for them.
///  - anything else (Shift_JIS, EUC-JP, ISO-8859-1, Windows-1252, ...):
///    transcoded with invalid/undef -> U+FFFD, so the text becomes the right
///    characters instead of being read as raw UTF-8 and mangled. Only
///    non-UTF-8 input pays for this.
///
/// An encoding Ruby cannot convert to UTF-8 comes back as its
/// `Encoding::ConverterNotFoundError`, returned rather than raised.
pub fn ruby_to_utf8_value(s: RString) -> Result<RString, Error> {
    const REPLACE: c_int = rb_sys::ruby_econv_flag_type::RUBY_ECONV_INVALID_REPLACE as c_int
        | rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_REPLACE as c_int;
    ruby_to_utf8(s, REPLACE)
}
