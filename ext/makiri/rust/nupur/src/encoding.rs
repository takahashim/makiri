//! A Ruby encoding, as the layer reads and converts Strings by it.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::rb::value::AsRawValue;
use crate::rb::{Error, RString};

/// A Ruby encoding - of a String, or resolved from a name or an `Encoding`.
///
/// Opaque so callers never hold the raw `rb_encoding*`, and the one place the
/// "which encodings are already UTF-8 bytes" rules live. Ruby's encodings are
/// process-lifetime objects, so a value of this type stays valid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Encoding(*mut nupur_sys::rb_encoding);

/* The indices of Ruby's three builtin encodings - the ones whose text is read
 * as UTF-8 bytes - asked once, at `Init_` ([`init`]), and only read after.
 * Ruby keeps them as constants, but hides them from extensions
 * (`ruby_preserved_encindex`), so they are asked rather than written here.
 * They are the one way the layer tells these three encodings: by index, as
 * a String's flags carry it, so an `Encoding` and a String are asked alike.
 * -1 is no encoding's index: read before `init`, no test matches, which
 * sends a String to Ruby's transcode - slower, never wrong. (No method runs
 * before `Init_`.) */
static UTF8: AtomicI32 = AtomicI32::new(-1);
static USASCII: AtomicI32 = AtomicI32::new(-1);
static ASCII8BIT: AtomicI32 = AtomicI32::new(-1);

/// Whether `index` is the builtin encoding `builtin` holds the index of.
#[inline]
fn is_builtin(builtin: &AtomicI32, index: c_int) -> bool {
    builtin.load(Ordering::Relaxed) == index
}

/// Ask Ruby the builtin encodings' indices, once: the first thing
/// `rb::method::run_init` does, before the extension's own `Init_` code.
pub(crate) fn init(ruby: &crate::rb::Ruby) {
    let gvl = ruby.gvl();
    // SAFETY: at `Init_`; each returns a constant.
    let (utf8, usascii, ascii8bit) = unsafe {
        (
            nupur_sys::read::rb_utf8_encindex(gvl),
            nupur_sys::read::rb_usascii_encindex(gvl),
            nupur_sys::read::rb_ascii8bit_encindex(gvl),
        )
    };
    UTF8.store(utf8, Ordering::Relaxed);
    USASCII.store(usascii, Ordering::Relaxed);
    ASCII8BIT.store(ascii8bit, Ordering::Relaxed);
}

/// Whether the encoding of index `index` is read as UTF-8 bytes with no
/// transcode: UTF-8, US-ASCII or ASCII-8BIT. The one definition of that set,
/// which [`Encoding::is_utf8_ascii_or_binary`] and a String's own
/// ([`RString::reads_as_utf8_bytes`](crate::rb::RString::reads_as_utf8_bytes))
/// both ask.
#[inline]
pub(crate) fn reads_as_utf8_index(index: c_int) -> bool {
    [&UTF8, &USASCII, &ASCII8BIT]
        .iter()
        .any(|b| is_builtin(b, index))
}

/// Whether the encoding of index `index` is UTF-8.
#[inline]
pub(crate) fn is_utf8_index(index: c_int) -> bool {
    is_builtin(&UTF8, index)
}

/// What a transcode ([`Encoding::encode`]) does with a byte sequence that is
/// invalid in the source, or a character the target has no form for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unconvertible {
    /// Raise Ruby's `EncodingError` (`InvalidByteSequenceError`,
    /// `UndefinedConversionError`).
    Raise,
    /// Replace either with U+FFFD (or `?` where the target has none).
    Replace,
    /// Write a character the target has no form for as a hex character
    /// reference (`&#x...;`); an invalid sequence still raises.
    HexCharRef,
}

impl Unconvertible {
    /// `rb_str_encode`'s flags for it.
    fn ecflags(self) -> c_int {
        use nupur_sys::ruby_econv_flag_type as f;
        match self {
            Unconvertible::Raise => 0,
            Unconvertible::Replace => {
                f::RUBY_ECONV_INVALID_REPLACE as c_int | f::RUBY_ECONV_UNDEF_REPLACE as c_int
            }
            Unconvertible::HexCharRef => f::RUBY_ECONV_UNDEF_HEX_CHARREF as c_int,
        }
    }
}

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
        Encoding(unsafe { nupur_sys::rb_enc_get(str.as_raw()) })
    }

    /// UTF-8. Takes the token, as every C call of the layer that has no Ruby
    /// value to show for the GVL does.
    pub fn utf8(ruby: &crate::rb::Ruby) -> Encoding {
        // SAFETY: Ruby's immutable global encoding.
        Encoding(unsafe { nupur_sys::read::rb_utf8_encoding(ruby.gvl()) })
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
        let Some(c) = crate::cname::CName::new(name) else {
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
    pub(crate) unsafe fn from_raw(p: *mut nupur_sys::rb_encoding) -> Encoding {
        Encoding(p)
    }

    /// The raw encoding, for a C call of the layer's that takes one.
    pub(crate) fn as_raw(self) -> *mut nupur_sys::rb_encoding {
        self.0
    }

    pub fn is_usascii(self) -> bool {
        is_builtin(&USASCII, self.index())
    }

    pub fn is_ascii8bit(self) -> bool {
        is_builtin(&ASCII8BIT, self.index())
    }

    /// The encoding's index (`rb_enc_to_index`), as a String's flags carry
    /// it.
    fn index(self) -> c_int {
        // SAFETY: one of Ruby's encodings, which live for the process; the
        // read of its index field raises nothing. An `Encoding` is made only
        // where a token or a Ruby value was, and is `!Send`: the GVL is held.
        unsafe { nupur_sys::read::rb_enc_to_index(nupur_sys::gvl::Gvl::assume(), self.0) }
    }

    /// UTF-8 or US-ASCII: text in it is already UTF-8, character for
    /// character. What a serialized String needs no transcoding into.
    pub fn is_utf8_or_ascii(self) -> bool {
        let index = self.index();
        is_utf8_index(index) || is_builtin(&USASCII, index)
    }

    /// UTF-8, US-ASCII or ASCII-8BIT: an encoding whose text is read as
    /// UTF-8 bytes with no transcode (ASCII-8BIT deliberately raw bytes).
    /// Anything else is transcoded to UTF-8 first. How the bytes are then
    /// checked is each reader's own: one repairs invalid UTF-8 to U+FFFD,
    /// another refuses it.
    ///
    /// For an encoding that is not a String's - a declared one, a given one.
    /// A String's own is [`RString::reads_as_utf8_bytes`](crate::rb::RString::reads_as_utf8_bytes),
    /// which reads its flags rather than look its encoding up.
    pub fn is_utf8_ascii_or_binary(self) -> bool {
        reads_as_utf8_index(self.index())
    }

    /// A copy of `str` tagged with this encoding instead of its own: its
    /// bytes read as this encoding's. Allocating, so a raise
    /// (`NoMemoryError`, or `ArgumentError` from the tag) is returned.
    pub fn relabel(self, str: RString) -> Result<RString, Error> {
        /* The copy is on this frame, where the conservative scan sees it. */
        let copy = crate::raw::str_dup(str)?;
        crate::raw::enc_associate(copy, self)
    }

    /// `str` transcoded to this encoding by Ruby (`rb_str_encode`), `on`
    /// deciding what an unconvertible sequence becomes. Raises - no
    /// converter at all, or a sequence `on` leaves to raise - and the raise
    /// comes back as `Err`.
    pub fn encode(self, str: RString, on: Unconvertible) -> Result<RString, Error> {
        crate::raw::str_encode(str, self, on.ecflags())
    }
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
