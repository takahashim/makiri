//! A Ruby String read into Makiri: the one copy every String crosses as,
//! the checked views ([`RubyText`], [`RubyData`]) and the contract checks.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use core::ops::Deref;

use crate::bridge::rb::Coderange;

use crate::bridge::rb::ReprValue;
use crate::bridge::rb::{Error, RString, Value};
use crate::bridge::ruby::makiri_error;

/// The shared owned buffer.
use crate::cbuf::OwnedBuf;
use crate::utf8_copy::{CopyError, FillError, Utf8Copy};

use super::*;
use crate::bridge::ruby::{name_arg, string_arg};

/* ---- the checked-text types ----
 *
 * Two owned-text types with ONE layout. The distinction is the contract, not
 * the shape: `text` has been checked for valid UTF-8 *and* no NUL, `data` for
 * UTF-8 only (the HTML data family may hold U+0000, like browsers). Keeping
 * them apart is what makes a name or engine string that took the data path a
 * type error rather than a silent one, so they stay two types here:
 * [`RubyText`] / [`RubyData`], one guard type with the contract as its
 * parameter. The parse source, which no contract checks, is [`HtmlSource`]. */

/// The contract a [`RubyStr`] was checked against. Uninhabited: types only.
pub enum TextContract {}
/// See [`TextContract`].
pub enum DataContract {}

mod sealed {
    pub trait Checked {}
    impl Checked for super::TextContract {}
    impl Checked for super::DataContract {}
}
use sealed::Checked;

/// Bytes copied out of a Ruby String and checked against `C`: [`RubyText`]
/// is valid UTF-8 with no NUL, [`RubyData`] is valid UTF-8 with NUL permitted
/// (the HTML data family). They are separate types because the contract is
/// the only thing that stops a data-family value from reaching an engine
/// input.
///
/// The view owns its bytes - a [`Utf8Copy`], checked to be UTF-8 by std's
/// validator over the copy itself, never by the String's cached coderange -
/// so nothing Ruby does after the copy (a GC, a mutation of the String, a
/// reallocation) can reach them, and a checked view is a plain `&str`
/// ([`Deref`]) for as long as it lives.
pub struct RubyStr<C> {
    bytes: Utf8Copy,
    contract: core::marker::PhantomData<C>,
}

pub type RubyText = RubyStr<TextContract>;
pub type RubyData = RubyStr<DataContract>;

/// Why [`RubyStr::acquire`] gave no view.
#[allow(
    clippy::large_enum_variant,
    reason = "an Error holds its message inline (`crate::msg::Msg`), so it is never boxed"
)]
enum Refusal<P> {
    /// The bytes failed the contract; `P` is the check's own answer.
    Check(P),
    /// The bytes could not be copied.
    Oom,
    /// The transcode to UTF-8 raised ([`utf8_strict`]): an `EncodingError`
    /// when the text has no UTF-8 form or holds bytes that are not its
    /// encoding's - or anything else that cut it short, which
    /// [`encoding_refusal`] hands back as it is.
    Encoding(Error),
}

impl<P> crate::falloc::Oom for Refusal<P> {
    #[inline]
    fn oom() -> Self {
        Refusal::Oom
    }
}

impl<C: Checked> RubyStr<C> {
    /// A checked view of `s`: read in its own encoding ([`utf8_strict`]), its
    /// bytes put to `check` (the contract's own rules: a NUL, a length), then
    /// copied and the copy checked to be UTF-8 - `not_utf8` when it is not.
    /// The order is the error's: a NUL is reported before invalid UTF-8.
    fn acquire<P>(
        ruby: &Ruby,
        s: RString,
        not_utf8: P,
        check: impl FnOnce(&[u8]) -> Option<P>,
    ) -> Result<Self, Refusal<P>> {
        /* Read in its own encoding: a String in another one is transcoded
         * first, and the copy and the check are of the UTF-8 it became. The
         * transcode is a fresh String on this frame, so the GC keeps it. */
        let s = utf8_strict(ruby, s).map_err(Refusal::Encoding)?;
        /* Copied first, then checked: `check` and the UTF-8 check both read
         * the copy, which is what the view holds. */
        let bytes = Utf8Copy::filled(s.len(), |dst| s.copy_to(ruby, 0, dst), check).map_err(
            |e| match e {
                FillError::Refused(problem) => Refusal::Check(problem),
                FillError::Copy(CopyError::NotUtf8) => Refusal::Check(not_utf8),
                FillError::Copy(CopyError::Oom) => Refusal::Oom,
            },
        )?;
        Ok(RubyStr {
            bytes,
            contract: core::marker::PhantomData,
        })
    }
}

impl<C: Checked> Deref for RubyStr<C> {
    type Target = str;

    fn deref(&self) -> &str {
        self.bytes.as_str()
    }
}

impl RubyText {
    /// The text as an engine input, borrowed for the guard's lifetime.
    pub(crate) fn text(&self) -> crate::text::VerifiedText<'_> {
        /* The bridge checked the text contract when it built `self`. */
        crate::text::VerifiedText::from_checked(self)
    }
}

/* ---- reading a String's bytes ---- */

/// The first `N` bytes of `s` (all of them when it is shorter), copied, with
/// how many there are: what a sniff of a document's start reads, owned.
pub(in crate::bridge) fn prefix<const N: usize>(ruby: &Ruby, s: RString) -> ([u8; N], usize) {
    let mut out = [0u8; N];
    let n = s.copy_to(ruby, 0, &mut out);
    (out, n)
}

/// A live Ruby String's bytes from `off` (none past its end), copied into an
/// owned buffer reserved for exactly them.
pub fn ruby_string_bytes_from(ruby: &Ruby, s: RString, off: usize) -> Result<OwnedBuf, Error> {
    let n = s.len().saturating_sub(off);
    /* Nothing runs between the reservation and the copy that could change
     * the String; `append_to` checks the room again, and refuses rather than
     * grows. */
    OwnedBuf::reserved(n, |v| s.append_to(ruby, off, v).is_ok())
        .ok_or_else(|| makiri_error("out of memory reading a Ruby string"))
}

/// A live Ruby String's raw bytes, copied into an owned buffer.
pub fn ruby_string_bytes(ruby: &Ruby, s: RString) -> Result<OwnedBuf, Error> {
    ruby_string_bytes_from(ruby, s, 0)
}

/* ---- the strict text contract ---- */

/// What the strict-text check found.
///
/// The enum itself lives in [`crate::cutf8`] beside the pure [`text_verdict`];
/// re-exported here so the bridge's callers keep naming it from this module.
pub use crate::cutf8::TextVerdict;

/// Check `bytes` against the strict contract, returning the specific
/// violation so each caller can map it to its own error surface (`Makiri::Error`,
/// `XML::SyntaxError`, or a reason string).
///
/// It resolves the String's cached coderange into an ordinary value, then
/// delegates the actual check to the pure [`crate::cutf8::text_verdict`]; the
/// logic is Ruby-free and testable under the always-compiled core.
///
/// `coderange_str` is consulted only for its CACHED coderange, which never
/// scans and never allocates.
pub fn text_check(coderange_str: RString, bytes: &[u8]) -> TextVerdict {
    /* The cached coderange reads flags; it never scans and never allocates. */
    crate::cutf8::text_verdict(bytes, ruby_str_known_valid_utf8(coderange_str))
}

/// Why a String in another encoding was refused ([`Refusal::Encoding`]).
const NOT_UTF8_CONVERTIBLE: &str = "could not be converted to UTF-8 from its encoding";

/// The error for a String whose transcode to UTF-8 raised `e`: an
/// `EncodingError` is the String's own fault, worded as the contract's
/// refusal; anything else - a `NoMemoryError`, an interrupt delivered while
/// Ruby loaded a converter - is not about the String, and is returned as it
/// was rather than reworded as one.
fn encoding_refusal(ruby: &Ruby, what: &str, e: Error) -> Error {
    if e.is_kind_of(ruby, crate::bridge::rb::Builtin::EncodingError) {
        text_error(what, NOT_UTF8_CONVERTIBLE)
    } else {
        e
    }
}

/// The error for a checked view whose bytes could not be copied.
fn oom_reading() -> Error {
    makiri_error("out of memory reading a Ruby string")
}

/// `Makiri::Error` with "<what> <problem>", the wording the C raised with.
fn text_error(what: &str, problem: &str) -> Error {
    makiri_error(format_args!("{what} {problem}"))
}

/// [`ruby_verified_text`] for an optional argument: `nil` is `None`.
pub fn ruby_verified_text_opt(
    ruby: &Ruby,
    in_: Value,
    what: &str,
) -> Result<Option<RubyText>, Error> {
    if in_.is_nil() {
        return Ok(None);
    }
    ruby_verified_text(ruby, in_, what).map(Some)
}

/// The `namespace_or_nil` argument of the namespaced APIs: `nil` and the empty
/// string ALIKE mean "no namespace", so everything below receives `None` or a
/// non-empty URI and never re-decides.
pub fn namespace_arg(ruby: &Ruby, in_: Value, what: &str) -> Result<Option<RubyText>, Error> {
    Ok(ruby_verified_text_opt(ruby, in_, what)?.filter(|t| !t.is_empty()))
}

/// Coerce to a String and enforce the strict contract (valid UTF-8, no NUL),
/// naming `what` in the error. The names-and-engine-input path.
pub fn ruby_verified_text(ruby: &Ruby, in_: Value, what: &str) -> Result<RubyText, Error> {
    verified_text_raising(ruby, in_, what, |m| makiri_error(m))
}

/// [`ruby_verified_text`] for a NAME given to a factory or a setter: the same
/// contract, but a NUL raises `ArgumentError` - the DOM's
/// InvalidCharacterError, which is what every other character a naming rule
/// refuses raises. It raised `Makiri::Error` from here, before any naming rule
/// ran, so a caller translating `ArgumentError` into a DOMException let that
/// one through. Invalid UTF-8 stays `Makiri::Error`: it breaks the String
/// contract every input shares, not a naming rule.
pub fn ruby_verified_name(ruby: &Ruby, in_: Value, what: &str) -> Result<RubyText, Error> {
    verified_text_raising(ruby, in_, what, |m| crate::bridge::ruby::arg_error(m))
}

/// The one acquisition behind [`ruby_verified_text`] and
/// [`ruby_verified_name`], which differ only in what a NUL raises: `on_nul`
/// is handed the message. Everything else - invalid UTF-8, a copy that ran out
/// of memory - is answered the same way for both.
fn verified_text_raising(
    ruby: &Ruby,
    in_: Value,
    what: &str,
    on_nul: fn(core::fmt::Arguments<'_>) -> Error,
) -> Result<RubyText, Error> {
    use crate::cutf8::TextVerdict;
    let s = name_arg(in_)?;
    RubyText::acquire(ruby, s, TextVerdict::InvalidUtf8, |b| {
        b.contains(&0).then_some(TextVerdict::HasNul)
    })
    .map_err(|r| match r {
        Refusal::Check(TextVerdict::HasNul) => on_nul(format_args!(
            "{what} {}",
            TextVerdict::HasNul.problem().unwrap_or_default()
        )),
        Refusal::Check(v) => text_error(what, v.problem().unwrap_or_default()),
        Refusal::Oom => oom_reading(),
        Refusal::Encoding(e) => encoding_refusal(ruby, what, e),
    })
}

/// Coerce to a String and enforce the DATA-family contract: invalid UTF-8 is
/// fatal, an interior NUL is not, so DOM data can hold U+0000 like browsers.
///
/// The same acquisition as [`ruby_verified_text`], with the data check - which
/// lets NUL through - in place of the text one.
pub fn ruby_verified_data(ruby: &Ruby, in_: Value, what: &str) -> Result<RubyData, Error> {
    let s = string_arg(in_)?;
    let not_utf8 = TextVerdict::InvalidUtf8.data_problem().unwrap_or_default();
    RubyData::acquire(ruby, s, not_utf8, |_| None).map_err(|r| match r {
        Refusal::Check(problem) => text_error(what, problem),
        Refusal::Oom => oom_reading(),
        Refusal::Encoding(e) => encoding_refusal(ruby, what, e),
    })
}

/// Whether Ruby ALREADY knows the String is valid UTF-8.
///
/// This reads the cached classification from the object's flags; it does not
/// scan (a scan would cost as much as running our own validator), so it only
/// wins when Ruby has the answer already. UNKNOWN or BROKEN returns false and
/// the caller validates or sanitises.
///
/// Only a hint for what stays safe code whatever it says - the HTML parse
/// (`HtmlSource`, which skips the sanitiser) and the XML input check. The
/// checked views no longer read it: a `&str` rests on `Utf8Copy`'s own check
/// of the copy, since a wrong coderange (another C extension's, or a Ruby bug
/// an attacker can reach) would otherwise be undefined behaviour here.
pub(in crate::bridge) fn ruby_str_known_valid_utf8(str: RString) -> bool {
    match str.enc_coderange() {
        /* Every byte < 0x80 in an ASCII-compatible encoding. */
        Coderange::SevenBit => true,
        /* Valid for its own encoding - which has to be UTF-8 for that to mean
         * valid UTF-8. */
        Coderange::Valid => Encoding::of(str) == Encoding::utf8(),
        _ => false,
    }
}

/// [`ruby_try_verified_text`] for two Strings at once (a `{prefix => uri}`
/// pair): both are live Strings.
pub fn ruby_try_verified_text_pair(
    ruby: &Ruby,
    a: RString,
    b: RString,
    max_bytes: usize,
) -> Result<(RubyText, RubyText), &'static str> {
    let a = ruby_try_verified_text(ruby, a, max_bytes)?;
    let b = ruby_try_verified_text(ruby, b, max_bytes)?;
    Ok((a, b))
}

/// The non-raising form: the checked view, or a static reason on rejection.
/// Nothing is coerced: `sv` is a String by type.
pub fn ruby_try_verified_text(
    ruby: &Ruby,
    sv: RString,
    max_bytes: usize,
) -> Result<RubyText, &'static str> {
    const TOO_LONG: &str = "string exceeds the maximum length";
    /* The cap is on the UTF-8 the String becomes, so it is measured after the
     * transcode: a UTF-16 String is twice its UTF-8 length. Refused before
     * the copy, so an over-long String is not copied whole only to be
     * refused; the check on the copy below stays authoritative. (`acquire`'s
     * own transcode is then a no-op on a String that is UTF-8 already.) */
    let sv = utf8_strict(ruby, sv).map_err(|_| NOT_UTF8_CONVERTIBLE)?;
    if sv.len() > max_bytes {
        return Err(TOO_LONG);
    }
    let not_utf8 = TextVerdict::InvalidUtf8.reason().unwrap_or_default();
    RubyText::acquire(ruby, sv, not_utf8, |b| {
        if b.len() > max_bytes {
            return Some(TOO_LONG);
        }
        b.contains(&0)
            .then(|| TextVerdict::HasNul.reason().unwrap_or_default())
    })
    .map_err(|r| match r {
        Refusal::Check(reason) => reason,
        Refusal::Oom => "could not be copied (out of memory)",
        Refusal::Encoding(_) => NOT_UTF8_CONVERTIBLE,
    })
}
