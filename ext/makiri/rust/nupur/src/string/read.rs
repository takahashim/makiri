//! Reading a Ruby String: by copying, never by a slice handed out
//! ([`RString::copy_to`], [`RString::append_to`]), and as text - in its own
//! encoding, transcoded to UTF-8 where it is not read as UTF-8 bytes
//! already, and checked to be UTF-8 on a copy ([`read_utf8`]).

#![allow(unsafe_code)]

use core::ffi::c_int;

use crate::encoding::{Encoding, Unconvertible};
use crate::rb::value::AsRawValue;
use crate::rb::{Error, RString, Ruby};

/// [`RString::append_to`]'s refusal: the buffer had too little spare room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoRoom;

impl RString {
    /// Copy the bytes from `offset` (none past the end) into `dst`, as many
    /// as both hold, and say how many.
    ///
    /// The String's buffer is read and copied out of in one step, with
    /// nothing run in between, and no slice of it is handed out: a caller
    /// owns what it reads, so nothing Ruby does afterwards - a GC, the String
    /// rewritten or grown - can reach it.
    #[inline]
    pub fn copy_to(self, _ruby: &Ruby, offset: usize, dst: &mut [u8]) -> usize {
        // SAFETY: read and copied out at once; nothing runs while it is held.
        let src = unsafe { self.as_slice() }.get(offset..).unwrap_or(&[]);
        let n = src.len().min(dst.len());
        if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
            d.copy_from_slice(s);
        }
        n
    }

    /// Append the bytes from `offset` (none past the end) to `v`, whose spare
    /// capacity must already hold them - the caller reserved it, fallibly -
    /// and say how many. Nothing is allocated here: when the room is short,
    /// `v` is left as it was and the answer is [`NoRoom`]. The read is
    /// [`copy_to`](Self::copy_to)'s, in one step after the room is checked.
    #[inline]
    pub fn append_to(self, _ruby: &Ruby, offset: usize, v: &mut Vec<u8>) -> Result<usize, NoRoom> {
        // SAFETY: as `copy_to` - read and copied out at once.
        let src = unsafe { self.as_slice() }.get(offset..).unwrap_or(&[]);
        if v.capacity() - v.len() < src.len() {
            return Err(NoRoom);
        }
        #[allow(
            clippy::disallowed_methods,
            reason = "the room was checked above, so it cannot grow"
        )]
        v.extend_from_slice(src);
        Ok(src.len())
    }
}

/* ---- the String's encoding, as a reader takes it ---- */

/// [`Encoding::reads_as_utf8_bytes`](crate::encoding::Encoding::reads_as_utf8_bytes) for `s`'s encoding, by index and
/// with no call on the common path: the index is read from the String's
/// flags, where Ruby keeps it inline for every encoding below
/// `RUBY_ENCODING_INLINE_MAX` (the builtin three are 0-2), and compared
/// with the builtin indices, looked up once. Every strict read asks, so it
/// is on the hot path: `rb_enc_get` + `rb_utf8_encoding` cost Makiri's
/// `Node#[]` ~13%.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raising_call_outside_protect,
        reason = "rb_enc_get_index raises for a File or a Symbol, as rb_enc_get; `s` is a String, and a String's index is an ivar read when it is not inline"
    )
)]
pub fn reads_as_utf8_bytes(_: &Ruby, s: RString) -> bool {
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

/// `s` as a UTF-8 String, transcoded by Ruby (`on` deciding what a character it cannot convert
/// becomes) unless its bytes
/// are read as UTF-8 already ([`reads_as_utf8_bytes`] -
/// UTF-8, US-ASCII, ASCII-8BIT), which costs one encoding comparison: no
/// transcode, no copy, no protected call. The transcode raises - no
/// converter at all (UTF-7 to UTF-8), or a character it cannot convert under
/// `on` - and the raise comes back as `Err`.
fn to_utf8(ruby: &Ruby, s: RString, on: Unconvertible) -> Result<RString, Error> {
    if reads_as_utf8_bytes(ruby, s) {
        return Ok(s);
    }
    Encoding::utf8(ruby).encode(s, on)
}

/// `s` as UTF-8 for a strict reader: transcoded with no replacement, so a
/// character with no UTF-8 form, or a byte that is not one of the encoding's,
/// is Ruby's `EncodingError` rather than a U+FFFD.
pub fn to_utf8_strict(ruby: &Ruby, s: RString) -> Result<RString, Error> {
    to_utf8(ruby, s, Unconvertible::Raise)
}

/// `s` as UTF-8 for a lenient reader: transcoded with every invalid or
/// undefined character replaced (U+FFFD), so the content survives. An
/// encoding Ruby has no converter for is still its `Err`.
pub fn to_utf8_replacing(ruby: &Ruby, s: RString) -> Result<RString, Error> {
    to_utf8(ruby, s, Unconvertible::Replace)
}

/// Whether Ruby already knows `s` to be valid UTF-8, from its cached
/// coderange: a read of its flags, which never scans and never allocates.
/// Only a hint - for code that stays safe whatever it says (a reader that
/// would otherwise repair or check the bytes itself). A wrong coderange
/// (another C extension's, a Ruby bug) must never decide what a `&str`
/// holds: that is [`crate::utf8_copy::Utf8Copy`]'s own check.
pub fn known_utf8(ruby: &Ruby, s: RString) -> bool {
    match s.enc_coderange() {
        /* Every byte < 0x80 in an ASCII-compatible encoding. */
        crate::rb::Coderange::SevenBit => true,
        /* Valid for its own encoding, which has to be UTF-8 for that to mean
         * valid UTF-8. */
        crate::rb::Coderange::Valid => Encoding::of(s) == Encoding::utf8(ruby),
        _ => false,
    }
}

/* ---- reading a String as checked UTF-8 ---- */

/// Why [`read_utf8`] gave no copy.
#[allow(
    clippy::large_enum_variant,
    reason = "an Error holds its message inline (`crate::msg::Msg`), so it is never boxed"
)]
pub enum ReadError<P> {
    /// The caller's own check turned the bytes down (a NUL).
    Refused(P),
    /// The UTF-8 is longer than the reader's cap ([`read_utf8_within`]).
    TooLong,
    /// The bytes are not UTF-8.
    NotUtf8,
    /// The String's encoding has no UTF-8 form for it: Ruby's transcode
    /// raised an `EncodingError`.
    NoUtf8Form,
    /// The copy could not be allocated.
    Oom,
    /// Something not about the String cut the transcode short
    /// (`NoMemoryError`, an interrupt, a `throw`), passed on as it was.
    Raised(Error),
}

/// `s`, read in its own encoding and copied as checked UTF-8: a String in
/// another encoding is transcoded first with no replacement
/// ([`to_utf8_strict`]), the copy of what it became is put
/// to `check` - a reader's own rules, reported ahead of invalid UTF-8 - and
/// then checked to be UTF-8 by std's validator over the copy itself, never by
/// the String's cached coderange. Nothing Ruby does afterwards reaches the
/// copy.
pub fn read_utf8<P>(
    ruby: &Ruby,
    s: RString,
    check: impl FnOnce(&[u8]) -> Option<P>,
) -> Result<crate::utf8_copy::Utf8Copy, ReadError<P>> {
    read_utf8_within(ruby, s, usize::MAX, check)
}

/// [`read_utf8`] under a cap of `max_bytes` on the UTF-8 the String becomes
/// (a UTF-16 String is about twice its UTF-8 length, so the cap is measured
/// after the transcode). An over-long String is refused before it is copied,
/// and the copy is held to the cap again, ahead of `check`.
pub fn read_utf8_within<P>(
    ruby: &Ruby,
    s: RString,
    max_bytes: usize,
    check: impl FnOnce(&[u8]) -> Option<P>,
) -> Result<crate::utf8_copy::Utf8Copy, ReadError<P>> {
    use crate::utf8_copy::{CopyError, FillError, Utf8Copy};
    /* The transcode is a fresh String on this frame, so the GC keeps it. An
     * `EncodingError` is the String's own fault; anything else - every exit
     * included, which no class names - is passed on. */
    let s = to_utf8_strict(ruby, s).map_err(|e| {
        match e.rescue(ruby, &[crate::rb::Builtin::EncodingError]) {
            Ok(_handled) => ReadError::NoUtf8Form,
            Err(e) => ReadError::Raised(e),
        }
    })?;
    if s.len() > max_bytes {
        return Err(ReadError::TooLong);
    }
    let checks = |b: &[u8]| {
        if b.len() > max_bytes {
            Some(ReadError::TooLong)
        } else {
            check(b).map(ReadError::Refused)
        }
    };
    Utf8Copy::from_source(&StrSource(ruby, s), checks).map_err(|e| match e {
        FillError::Refused(r) => r,
        FillError::Copy(CopyError::NotUtf8) => ReadError::NotUtf8,
        FillError::Copy(CopyError::Oom) => ReadError::Oom,
    })
}

/// A live String as what a [`Utf8Copy`](crate::utf8_copy::Utf8Copy) copies
/// from: the copying reads, which hand no slice of the String out.
struct StrSource<'a>(&'a Ruby, RString);

impl crate::utf8_copy::Source for StrSource<'_> {
    fn byte_len(&self) -> usize {
        self.1.len()
    }

    fn copy_into(&self, dst: &mut [u8]) -> usize {
        self.1.copy_to(self.0, 0, dst)
    }

    fn append_into(&self, v: &mut Vec<u8>) -> bool {
        self.1.append_to(self.0, 0, v).is_ok()
    }
}
