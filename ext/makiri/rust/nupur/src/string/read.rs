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
    pub fn copy_to(self, offset: usize, dst: &mut [u8]) -> usize {
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
    pub fn append_to(self, offset: usize, v: &mut Vec<u8>) -> Result<usize, NoRoom> {
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

    /* ---- the String's encoding, as a reader takes it ---- */

    /// Whether this String's text is read as UTF-8 bytes with no transcode -
    /// its encoding UTF-8, US-ASCII or ASCII-8BIT
    /// ([`Encoding::is_utf8_ascii_or_binary`]) - asked of the String itself, by
    /// index and with no call on the common path: the index is read from the String's
    /// flags, where Ruby keeps it inline for every encoding below
    /// `RUBY_ENCODING_INLINE_MAX` (the builtin three are 0-2), and compared
    /// with the builtin indices (`encoding`'s one table of them). Every strict read asks, so it
    /// is on the hot path: `rb_enc_get` + `rb_utf8_encoding` cost Makiri's
    /// `Node#[]` ~13%.
    pub fn reads_as_utf8_bytes(self) -> bool {
        crate::encoding::reads_as_utf8_index(self.encoding_index())
    }

    /// The String's encoding index, read from its flags where Ruby keeps it
    /// inline (every encoding below `RUBY_ENCODING_INLINE_MAX`), and looked
    /// up only past that.
    #[inline]
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raising_call_outside_protect,
            reason = "rb_enc_get_index raises for a File or a Symbol, as rb_enc_get; `s` is a String, and a String's index is an ivar read when it is not inline"
        )
    )]
    fn encoding_index(self) -> c_int {
        use rb_sys::ruby_encoding_consts::{
            RUBY_ENCODING_INLINE_MAX, RUBY_ENCODING_MASK, RUBY_ENCODING_SHIFT,
        };
        // SAFETY: `self` is a live String - a heap object, whose flags word is
        // the first field of its `RBasic` and readable; reading it runs no Ruby.
        let flags = unsafe { (*(self.as_raw() as *const rb_sys::RBasic)).flags };
        /* Seven bits, which a `c_int` holds. */
        let bits =
            (crate::int::word(flags) & RUBY_ENCODING_MASK as usize) >> RUBY_ENCODING_SHIFT as u32;
        let inline = c_int::try_from(bits).unwrap_or(c_int::MAX);
        if inline == RUBY_ENCODING_INLINE_MAX as c_int {
            // SAFETY: as above; the lookup reads an instance variable and
            // raises nothing.
            unsafe { rb_sys::rb_enc_get_index(self.as_raw()) }
        } else {
            inline
        }
    }

    /// Whether Ruby already knows this String to be valid UTF-8, from its
    /// cached coderange: a read of its flags, which never scans and never
    /// allocates. Only a hint - for code that stays safe whatever it says (a
    /// reader that would otherwise repair or check the bytes itself). A wrong
    /// coderange (another C extension's, a Ruby bug) must never decide what a
    /// `&str` holds: that is [`crate::utf8_copy::Utf8Copy`]'s own check.
    pub fn known_utf8(self) -> bool {
        match self.enc_coderange() {
            /* Every byte < 0x80 in an ASCII-compatible encoding. */
            crate::rb::Coderange::SevenBit => true,
            /* Valid for its own encoding, which has to be UTF-8 for that to
             * mean valid UTF-8 - asked by index, from the flags. */
            crate::rb::Coderange::Valid => crate::encoding::is_utf8_index(self.encoding_index()),
            _ => false,
        }
    }
}

/// What [`to_utf8`] made of a String.
#[allow(
    clippy::large_enum_variant,
    reason = "an Error holds its message inline (`crate::msg::Msg`), so it is never boxed"
)]
pub enum Transcode {
    /// The String itself: its bytes are read as UTF-8 bytes already, and are
    /// whatever they are - checking them is the reader's.
    AsIs(RString),
    /// The new String the transcode made (on this frame, where the GC keeps
    /// it). Under [`Unconvertible::Replace`] it is valid UTF-8, every
    /// invalid or unmappable sequence replaced.
    Transcoded(RString),
    /// The String's own fault: the transcode raised an `EncodingError` - no
    /// converter, an invalid byte sequence, a character with no UTF-8 form
    /// under [`Unconvertible::Raise`]. Handled: the caller's `$!` is as it
    /// was, and the exception is Ruby's own, to word another error with, to
    /// raise as it is, or to drop - it is an exception the String caused,
    /// never an exit, so dropping it loses nothing.
    Fault(Error),
}

/// `s` as UTF-8 text, read as `read_as`'s bytes - the String's own encoding
/// for `None` - and transcoded by Ruby where that encoding is not read as
/// UTF-8 bytes already (UTF-8, US-ASCII, ASCII-8BIT), `on` deciding what an
/// unconvertible sequence becomes.
///
/// The one place a String's transcode is classified: an `EncodingError` is
/// the String's fault ([`Transcode::Fault`]), handled here with the caller's
/// `$!` put back; anything else - `NoMemoryError`, an interrupt, every exit,
/// which no class names - is `Err`, to be passed on. A String already read as
/// UTF-8 bytes costs a read of its flags (`read_as` `None`) or one encoding
/// comparison: no protected call, no call into libruby at all.
pub fn to_utf8(
    ruby: &Ruby,
    s: RString,
    read_as: Option<Encoding>,
    on: Unconvertible,
) -> Result<Transcode, Error> {
    let as_utf8 = match read_as {
        None => s.reads_as_utf8_bytes(),
        Some(enc) => enc.is_utf8_ascii_or_binary(),
    };
    if as_utf8 {
        return Ok(Transcode::AsIs(s));
    }
    /* `$!` is read and put back only here, where a transcode runs: it costs
     * a call into libruby, and a raise that is passed on replaces it anyway. */
    let made = crate::raw::keeping_errinfo(|| {
        let input = match read_as {
            Some(enc) if enc != Encoding::of(s) => enc.relabel(s)?,
            _ => s,
        };
        Encoding::utf8(ruby).encode(input, on)
    });
    match made {
        Ok(utf8) => Ok(Transcode::Transcoded(utf8)),
        Err(e) if e.is_kind_of(ruby, crate::rb::Builtin::EncodingError) => Ok(Transcode::Fault(e)),
        Err(e) => Err(e),
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
/// another encoding is transcoded first with no replacement ([`to_utf8`]),
/// the copy of what it became is put
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
    /* A transcode's UTF-8 is at least a quarter of its source - no encoding
     * takes more than four bytes a character, nor UTF-8 fewer than one - so a
     * String that cannot fit is refused before it is transcoded, whatever
     * its size. (A stateful encoding's escapes make no character, so one
     * padded with them past four times the cap is refused even so: the
     * closed direction.) */
    if s.len() / 4 > max_bytes {
        return Err(ReadError::TooLong);
    }
    /* The common case, a String already read as UTF-8 bytes, asks its flags
     * here and skips `to_utf8`'s `Result`, whose `Err` (an `Error`, its
     * message inline) is a few hundred bytes to move on every read. */
    let s = if s.reads_as_utf8_bytes() {
        s
    } else {
        match to_utf8(ruby, s, None, Unconvertible::Raise).map_err(ReadError::Raised)? {
            Transcode::AsIs(s) | Transcode::Transcoded(s) => s,
            Transcode::Fault(_handled) => return Err(ReadError::NoUtf8Form),
        }
    };
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
    Utf8Copy::from_source(&StrSource(s), checks).map_err(|e| match e {
        FillError::Refused(r) => r,
        FillError::Copy(CopyError::NotUtf8) => ReadError::NotUtf8,
        FillError::Copy(CopyError::Oom) => ReadError::Oom,
    })
}

/// A live String as what a [`Utf8Copy`](crate::utf8_copy::Utf8Copy) copies
/// from: the copying reads, which hand no slice of the String out.
struct StrSource(RString);

impl crate::utf8_copy::Source for StrSource {
    fn byte_len(&self) -> usize {
        self.0.len()
    }

    fn copy_into(&self, dst: &mut [u8]) -> usize {
        self.0.copy_to(0, dst)
    }

    fn append_into(&self, v: &mut Vec<u8>) -> bool {
        self.0.append_to(0, v).is_ok()
    }
}
