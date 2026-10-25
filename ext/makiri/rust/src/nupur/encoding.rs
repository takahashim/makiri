//! A Ruby encoding, as the layer reads and converts Strings by it.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use crate::nupur::rb::value::AsRawValue;
use crate::nupur::rb::{Error, RString};

/// A Ruby encoding - of a String, or resolved from a name or an `Encoding`.
///
/// Opaque so callers never hold the raw `rb_encoding*`, and the one place the
/// "which encodings are already UTF-8 bytes" rules live. Ruby's encodings are
/// process-lifetime objects, so a value of this type stays valid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Encoding(*mut rb_sys::rb_encoding);

impl Encoding {
    /// The encoding's name, as Ruby spells it.
    pub fn name(self) -> &'static [u8] {
        // SAFETY: a live encoding's name is a NUL-terminated C string that
        // lives as long as the encoding, which is the process.
        unsafe { core::ffi::CStr::from_ptr((*self.0).name) }.to_bytes()
    }

    /// The encoding `str` is tagged with.
    pub(crate) fn of(str: RString) -> Encoding {
        // SAFETY: a live String, by type; reading its tag runs no Ruby.
        Encoding(unsafe { rb_sys::rb_enc_get(str.as_raw()) })
    }

    pub(crate) fn utf8() -> Encoding {
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
    /// does a lookup whose autoload failed (`LoadError`, `NoMemoryError`): the
    /// caller then refuses the name as unknown, never reads with another.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            discarded_ruby_error,
            reason = "a lookup that fails is an unknown name, which the caller refuses"
        )
    )]
    pub(crate) fn find(name: &[u8]) -> Option<Encoding> {
        let mut buf = [0u8; 64];
        if name.is_empty() || name.len() >= buf.len() || name.contains(&0) {
            return None;
        }
        buf[..name.len()].copy_from_slice(name);
        let c = core::ffi::CStr::from_bytes_with_nul(&buf[..name.len() + 1]).ok()?;
        crate::nupur::raw::enc_find(&crate::nupur::helpers::gvl_ruby(), c)
            .ok()
            .flatten()
    }

    /// An encoding from Ruby's pointer to it.
    ///
    /// # Safety
    /// `p` is one of Ruby's encodings (which live for the process).
    pub(in crate::nupur) unsafe fn from_raw(p: *mut rb_sys::rb_encoding) -> Encoding {
        Encoding(p)
    }

    /// The raw encoding, for a C call in the bridge that takes one.
    pub(in crate::nupur) fn as_raw(self) -> *mut rb_sys::rb_encoding {
        self.0
    }

    pub(crate) fn is_usascii(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_usascii_encoding() }
    }

    pub(crate) fn is_ascii8bit(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_ascii8bit_encoding() }
    }

    /// UTF-8 or US-ASCII: text in it is already UTF-8, character for
    /// character. What a serialized String needs no transcoding into.
    pub(crate) fn is_utf8_compatible(self) -> bool {
        self == Encoding::utf8() || self.is_usascii()
    }

    /// Whether text that is already UTF-8/US-ASCII needs hex-character-reference
    /// transcoding to this encoding (that is, it is something else).
    pub fn needs_transcode(self) -> bool {
        !self.is_utf8_compatible()
    }

    /// Whether input in this encoding is read as UTF-8 bytes with no
    /// transcode: UTF-8, US-ASCII, or ASCII-8BIT - deliberately raw bytes.
    /// Anything else is transcoded to UTF-8 first. How the bytes are then
    /// checked is each reader's own: HTML repairs invalid UTF-8 to U+FFFD
    /// (`ruby_to_utf8`), XML refuses it (`xml_decode`).
    pub(crate) fn reads_as_utf8_bytes(self) -> bool {
        self.is_utf8_compatible() || self.is_ascii8bit()
    }

    /// A copy of `str` tagged with this encoding instead of its own: its
    /// bytes read as this encoding's. Allocating, so a raise
    /// (`NoMemoryError`, or `ArgumentError` from the tag) is returned.
    pub(crate) fn relabel(self, str: RString) -> Result<RString, Error> {
        /* The copy is on this frame, where the conservative scan sees it. */
        let copy = crate::nupur::raw::str_dup(str)?;
        crate::nupur::raw::enc_associate(copy, self)
    }

    /// `str` transcoded to this encoding, a character it cannot represent
    /// becoming a hex character reference. A transcoding failure is returned,
    /// not raised.
    pub fn encode_charref(self, str: RString) -> Result<RString, Error> {
        const UNDEF_HEX_CHARREF: c_int =
            rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_HEX_CHARREF as c_int;
        crate::nupur::raw::str_encode(str, self, UNDEF_HEX_CHARREF)
    }
}
