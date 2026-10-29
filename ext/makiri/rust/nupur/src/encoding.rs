//! A Ruby encoding, as the layer reads and converts Strings by it.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use crate::rb::value::AsRawValue;
use crate::rb::{Error, RString};

/// A Ruby encoding - of a String, or resolved from a name or an `Encoding`.
///
/// Opaque so callers never hold the raw `rb_encoding*`, and the one place the
/// "which encodings are already UTF-8 bytes" rules live. Ruby's encodings are
/// process-lifetime objects, so a value of this type stays valid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Encoding(*mut rb_sys::rb_encoding);

/// `rb_str_encode`'s flags to replace, with U+FFFD (or `?`), an invalid byte
/// sequence or a character the target has no form for, rather than raise.
pub const ECONV_REPLACE: c_int = rb_sys::ruby_econv_flag_type::RUBY_ECONV_INVALID_REPLACE as c_int
    | rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_REPLACE as c_int;

impl Encoding {
    /// The encoding's name, as Ruby spells it.
    pub fn name(self) -> &'static [u8] {
        // SAFETY: a live encoding's name is a NUL-terminated C string that
        // lives as long as the encoding, which is the process.
        unsafe { core::ffi::CStr::from_ptr((*self.0).name) }.to_bytes()
    }

    /// The encoding `str` is tagged with.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raising_call_outside_protect,
            reason = "rb_enc_get raises for a File (a method call) or a Symbol (a String made); `str` is a String, whose tag it reads"
        )
    )]
    pub fn of(str: RString) -> Encoding {
        // SAFETY: a live String, by type; reading its tag runs no Ruby.
        Encoding(unsafe { rb_sys::rb_enc_get(str.as_raw()) })
    }

    /// UTF-8. Takes the token, as every C call of the layer that has no Ruby
    /// value to show for the GVL does.
    pub fn utf8(_: &crate::rb::Ruby) -> Encoding {
        // SAFETY: Ruby's immutable global encoding.
        Encoding(unsafe { rb_sys::rb_utf8_encoding() })
    }

    /// The encoding Ruby knows by `name`, or None for a name it does not
    /// know - not an error. MAY AUTOLOAD the encoding, a Ruby allocation and so
    /// a GC point: call it with no borrow of a String's bytes held.
    ///
    /// `rb_enc_find` wants a C string and the name is bytes, so the NUL is
    /// added here. A name too long to fit, or holding a NUL, is not one Ruby
    /// knows, so it reads as None rather than being truncated at the NUL. So
    /// does a lookup Ruby refuses (`LoadError`, `ArgumentError`): the caller
    /// then refuses the name as unknown, never reads with another.
    ///
    /// Any other `Err` is passed on: what reaches the autoload - a thread
    /// kill, a `throw`, `NoMemoryError` - is not an answer about the name,
    /// and read as one it would be lost (a kill's thread would run on).
    pub fn find(ruby: &crate::rb::Ruby, name: &[u8]) -> Result<Option<Encoding>, crate::rb::Error> {
        if name.is_empty() {
            return Ok(None);
        }
        let Some(c) = crate::rb::class::CName::new(name) else {
            return Ok(None);
        };
        use crate::rb::Builtin::{ArgumentError, LoadError};
        match crate::raw::keeping_errinfo(|| crate::raw::enc_find(ruby, &c)) {
            Ok(found) => Ok(found),
            Err(e) => {
                e.rescue(ruby, &[LoadError, ArgumentError])?;
                Ok(None)
            }
        }
    }

    /// An encoding from Ruby's pointer to it.
    ///
    /// # Safety
    /// `p` is one of Ruby's encodings (which live for the process).
    pub(crate) unsafe fn from_raw(p: *mut rb_sys::rb_encoding) -> Encoding {
        Encoding(p)
    }

    /// The raw encoding, for a C call of the layer's that takes one.
    pub(crate) fn as_raw(self) -> *mut rb_sys::rb_encoding {
        self.0
    }

    pub fn is_usascii(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_usascii_encoding() }
    }

    pub fn is_ascii8bit(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_ascii8bit_encoding() }
    }

    /// UTF-8 or US-ASCII: text in it is already UTF-8, character for
    /// character. What a serialized String needs no transcoding into.
    pub fn is_utf8_compatible(self, ruby: &crate::rb::Ruby) -> bool {
        self == Encoding::utf8(ruby) || self.is_usascii()
    }

    /// Whether text that is already UTF-8/US-ASCII needs hex-character-reference
    /// transcoding to this encoding (that is, it is something else).
    pub fn needs_transcode(self, ruby: &crate::rb::Ruby) -> bool {
        !self.is_utf8_compatible(ruby)
    }

    /// Whether input in this encoding is read as UTF-8 bytes with no
    /// transcode: UTF-8, US-ASCII, or ASCII-8BIT - deliberately raw bytes.
    /// Anything else is transcoded to UTF-8 first. How the bytes are then
    /// checked is each reader's own: HTML repairs invalid UTF-8 to U+FFFD
    /// - and the next reader may refuse it.
    pub fn reads_as_utf8_bytes(self, ruby: &crate::rb::Ruby) -> bool {
        self.is_utf8_compatible(ruby) || self.is_ascii8bit()
    }

    /// [`Encoding::reads_as_utf8_bytes`] for `s`'s encoding, by index and
    /// with no call on the common path: the index is read from the String's
    /// flags, where Ruby keeps it inline for every encoding below
    /// `RUBY_ENCODING_INLINE_MAX` (the builtin three are 0-2), and compared
    /// with the builtin indices, looked up once. Every strict read (a
    /// selector, an attribute name or value) asks, and `rb_enc_get` +
    /// `rb_utf8_encoding` cost `Node#[]` ~13%.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raising_call_outside_protect,
            reason = "rb_enc_get_index raises for a File or a Symbol, as rb_enc_get; `s` is a String, and a String's index is an ivar read when it is not inline"
        )
    )]
    pub fn string_reads_as_utf8_bytes(_: &crate::rb::Ruby, s: RString) -> bool {
        use rb_sys::ruby_encoding_consts::{
            RUBY_ENCODING_INLINE_MAX, RUBY_ENCODING_MASK, RUBY_ENCODING_SHIFT,
        };
        static BUILTIN: std::sync::OnceLock<[c_int; 3]> = std::sync::OnceLock::new();
        // SAFETY: `s` is a live String - a heap object, whose flags word is
        // the first field of its `RBasic` and readable; reading it runs no Ruby.
        let flags = unsafe { (*(s.as_raw() as *const rb_sys::RBasic)).flags };
        /* Seven bits, which a `c_int` holds. */
        let bits =
            (crate::int::word(flags) & RUBY_ENCODING_MASK as usize) >> RUBY_ENCODING_SHIFT as u32;
        let inline = c_int::try_from(bits).unwrap_or(c_int::MAX);
        let index = if inline == RUBY_ENCODING_INLINE_MAX as c_int {
            // SAFETY: as above; the lookup reads an instance variable and
            // raises nothing.
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

    /// A copy of `str` tagged with this encoding instead of its own: its
    /// bytes read as this encoding's. Allocating, so a raise
    /// (`NoMemoryError`, or `ArgumentError` from the tag) is returned.
    pub fn relabel(self, str: RString) -> Result<RString, Error> {
        /* The copy is on this frame, where the conservative scan sees it. */
        let copy = crate::raw::str_dup(str)?;
        crate::raw::enc_associate(copy, self)
    }

    /// `str` transcoded to this encoding, a character it cannot represent
    /// becoming a hex character reference. A transcoding failure is returned,
    /// not raised.
    pub fn encode_charref(self, str: RString) -> Result<RString, Error> {
        const UNDEF_HEX_CHARREF: c_int =
            rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_HEX_CHARREF as c_int;
        crate::raw::str_encode(str, self, UNDEF_HEX_CHARREF)
    }
}

/* ---- reading a String as UTF-8 ---- */

/// Is `e`, raised while reading a String as UTF-8, the String's own fault -
/// an `EncodingError` (an undefined conversion, an invalid byte sequence, no
/// converter)? Anything else (`NoMemoryError`, an interrupt delivered while
/// Ruby loaded a converter) is not about the String, and its reader passes
/// it on as it was.
pub fn is_encoding_fault(ruby: &crate::rb::Ruby, e: &Error) -> bool {
    e.is_kind_of(ruby, crate::rb::Builtin::EncodingError)
}

/// `s` as a UTF-8 String, transcoded by Ruby with `ecflags` unless its bytes
/// are read as UTF-8 already ([`Encoding::string_reads_as_utf8_bytes`] -
/// UTF-8, US-ASCII, ASCII-8BIT), which costs one encoding comparison: no
/// transcode, no copy, no protected call. The transcode raises - no
/// converter at all (UTF-7 to UTF-8), or a character it cannot convert under
/// `ecflags` - and the raise comes back as `Err`.
fn to_utf8(ruby: &crate::rb::Ruby, s: RString, ecflags: c_int) -> Result<RString, Error> {
    if Encoding::string_reads_as_utf8_bytes(ruby, s) {
        return Ok(s);
    }
    crate::raw::str_encode(s, Encoding::utf8(ruby), ecflags)
}

/// `s` as UTF-8 for a strict reader: transcoded with no replacement, so a
/// character with no UTF-8 form, or a byte that is not one of the encoding's,
/// is Ruby's `EncodingError` rather than a U+FFFD.
pub fn to_utf8_strict(ruby: &crate::rb::Ruby, s: RString) -> Result<RString, Error> {
    to_utf8(ruby, s, 0)
}

/// `s` as UTF-8 for a lenient reader: transcoded with every invalid or
/// undefined character replaced (U+FFFD), so the content survives. An
/// encoding Ruby has no converter for is still its `Err`.
pub fn to_utf8_replacing(ruby: &crate::rb::Ruby, s: RString) -> Result<RString, Error> {
    to_utf8(ruby, s, ECONV_REPLACE)
}

/// The encoding `v` names (an `Encoding`, or a String or Symbol naming one),
/// or the error Ruby's own lookup raises: `ArgumentError` for an unknown
/// name. Anything else is a `TypeError` before the lookup, which would run
/// its `to_str`.
pub fn to_encoding(ruby: &crate::rb::Ruby, v: crate::rb::Value) -> Result<Encoding, Error> {
    use crate::rb::ReprValue;
    let v = if v.is_kind_of(ruby.class_encoding()) {
        v
    } else {
        crate::args::name_arg(v)?.as_value()
    };
    crate::raw::to_encoding(ruby, v)
}

/// Whether Ruby already knows `s` to be valid UTF-8, from its cached
/// coderange: a read of its flags, which never scans and never allocates.
/// Only a hint - for code that stays safe whatever it says (a reader that
/// would otherwise repair or check the bytes itself). A wrong coderange
/// (another C extension's, a Ruby bug) must never decide what a `&str`
/// holds: that is [`crate::utf8_copy::Utf8Copy`]'s own check.
pub fn known_utf8(ruby: &crate::rb::Ruby, s: RString) -> bool {
    match s.enc_coderange() {
        /* Every byte < 0x80 in an ASCII-compatible encoding. */
        crate::rb::Coderange::SevenBit => true,
        /* Valid for its own encoding, which has to be UTF-8 for that to mean
         * valid UTF-8. */
        crate::rb::Coderange::Valid => Encoding::of(s) == Encoding::utf8(ruby),
        _ => false,
    }
}
