//! A Ruby String read into Makiri: the one copy every String crosses as,
//! the checked views ([`RubyText`], [`RubyData`]) and the contract checks.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use core::ops::Deref;

use crate::bridge::rb::ReprValue;
use crate::bridge::rb::{Error, RString, Value};
use crate::bridge::ruby::makiri_error;

use crate::nupur::string::{read_utf8_within, ReadError};
use crate::nupur::utf8_copy::Utf8Copy;

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

impl<C: Checked> RubyStr<C> {
    /// A checked view of `s` (`nupur::string::read_utf8`): read in its own
    /// encoding, held to `max_bytes` when there is a cap
    /// (`read_utf8_within`), its copy put to `check` (the contract's own
    /// rule: a NUL) and then checked to be UTF-8. The order is the error's: a
    /// NUL is reported before invalid UTF-8.
    fn acquire<P>(
        ruby: &Ruby,
        s: RString,
        max_bytes: Option<usize>,
        check: impl FnOnce(&[u8]) -> Option<P>,
    ) -> Result<Self, ReadError<P>> {
        /* No cap is `read_utf8`'s own: a cap of `usize::MAX`. */
        let bytes = read_utf8_within(ruby, s, max_bytes.unwrap_or(usize::MAX), check)?;
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
    crate::cutf8::text_verdict(bytes, coderange_str.known_utf8())
}

/// Why a String in another encoding was refused ([`ReadError::NoUtf8Form`]).
const NOT_UTF8_CONVERTIBLE: &str = "could not be converted to UTF-8 from its encoding";

/// The error for a checked view whose bytes could not be copied.
fn oom_reading() -> Error {
    makiri_error("out of memory reading a Ruby string")
}

/// `Makiri::Error` with "<what> <problem>", the wording the C raised with.
fn text_error(what: &str, problem: &str) -> Error {
    makiri_error(format_args!("{what} {problem}"))
}

/// The error for a checked view `nupur::string::read_utf8` refused: what each reader words
/// its own way - its check's refusal, invalid UTF-8 - by `refused` and
/// `not_utf8`, and the rest alike. With no cap `TooLong` cannot come back.
fn read_error<P>(
    r: ReadError<P>,
    what: &str,
    refused: impl FnOnce(P) -> Error,
    not_utf8: &str,
) -> Error {
    match r {
        ReadError::Refused(p) => refused(p),
        ReadError::NotUtf8 => text_error(what, not_utf8),
        ReadError::TooLong => text_error(what, TOO_LONG),
        ReadError::NoUtf8Form => text_error(what, NOT_UTF8_CONVERTIBLE),
        ReadError::Oom => oom_reading(),
        ReadError::Raised(e) => e,
    }
}

/// Why a String over a reader's cap was refused ([`ReadError::TooLong`]).
const TOO_LONG: &str = "string exceeds the maximum length";

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
    RubyText::acquire(ruby, s, None, |b| {
        b.contains(&0).then_some(TextVerdict::HasNul)
    })
    .map_err(|r| {
        read_error(
            r,
            what,
            |v| on_nul(format_args!("{what} {}", v.problem().unwrap_or_default())),
            TextVerdict::InvalidUtf8.problem().unwrap_or_default(),
        )
    })
}

/// Coerce to a String and enforce the DATA-family contract: invalid UTF-8 is
/// fatal, an interior NUL is not, so DOM data can hold U+0000 like browsers.
///
/// The same acquisition as [`ruby_verified_text`], with the data check - which
/// lets NUL through - in place of the text one.
pub fn ruby_verified_data(ruby: &Ruby, in_: Value, what: &str) -> Result<RubyData, Error> {
    let s = string_arg(in_)?;
    RubyData::acquire(ruby, s, None, |_| None::<core::convert::Infallible>).map_err(|r| {
        read_error(
            r,
            what,
            |never| match never {},
            TextVerdict::InvalidUtf8.data_problem().unwrap_or_default(),
        )
    })
}

/// Why [`ruby_try_verified_text`] gave no view: the String broke the
/// contract - a static reason, for the caller to word in its own error - or
/// something else cut the read short (`NoMemoryError`, an interrupt
/// delivered while Ruby loaded a converter, a `throw`), which is not about
/// the String and is passed on as it was.
#[allow(
    clippy::large_enum_variant,
    reason = "an Error holds its message inline (`crate::msg::Msg`), so it is never boxed"
)]
pub enum TextRefusal {
    Invalid(&'static str),
    Raised(Error),
}

impl TextRefusal {
    /// The error to return: `word`'s for a reason, the raise itself
    /// otherwise.
    pub fn into_error(self, word: impl FnOnce(&'static str) -> Error) -> Error {
        match self {
            TextRefusal::Invalid(reason) => word(reason),
            TextRefusal::Raised(e) => e,
        }
    }
}

/// [`ruby_try_verified_text`] for two Strings at once (a `{prefix => uri}`
/// pair): both are live Strings.
pub fn ruby_try_verified_text_pair(
    ruby: &Ruby,
    a: RString,
    b: RString,
    max_bytes: usize,
) -> Result<(RubyText, RubyText), TextRefusal> {
    let a = ruby_try_verified_text(ruby, a, max_bytes)?;
    let b = ruby_try_verified_text(ruby, b, max_bytes)?;
    Ok((a, b))
}

/// The checked view under a length cap, or why there is none
/// ([`TextRefusal`]). Nothing is coerced: `sv` is a String by type.
pub fn ruby_try_verified_text(
    ruby: &Ruby,
    sv: RString,
    max_bytes: usize,
) -> Result<RubyText, TextRefusal> {
    RubyText::acquire(ruby, sv, Some(max_bytes), |b| {
        b.contains(&0)
            .then(|| TextVerdict::HasNul.reason().unwrap_or_default())
    })
    .map_err(|r| match r {
        ReadError::Refused(reason) => TextRefusal::Invalid(reason),
        ReadError::TooLong => TextRefusal::Invalid(TOO_LONG),
        ReadError::NotUtf8 => {
            TextRefusal::Invalid(TextVerdict::InvalidUtf8.reason().unwrap_or_default())
        }
        ReadError::NoUtf8Form => TextRefusal::Invalid(NOT_UTF8_CONVERTIBLE),
        ReadError::Oom => TextRefusal::Invalid("could not be copied (out of memory)"),
        ReadError::Raised(e) => TextRefusal::Raised(e),
    })
}
