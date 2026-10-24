//! A Ruby String read into Makiri: the one copy every String crosses as,
//! the checked views ([`RubyText`], [`RubyData`]) and the contract checks.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ops::Deref;

use crate::bridge::rb::Coderange;

use crate::bridge::rb::value::AsRawValue;
use crate::bridge::rb::ReprValue;
use crate::bridge::rb::{Error, RString, Value};
use crate::bridge::ruby::makiri_error;
use rb_sys::StableApiDefinition;

/// The shared owned buffer.
use crate::cbuf::OwnedBuf;
use crate::falloc::OomOption;

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

/// Up to this many bytes a copy lives inside the view, on the stack.
const INLINE: usize = 64;

/// A copy of a String's bytes: inline when short, else a `falloc` allocation.
enum Bytes {
    Inline { len: u8, buf: [u8; INLINE] },
    Heap(OwnedBuf),
}

impl Bytes {
    /// A copy of `src`; `None` when the heap copy could not be allocated.
    fn copy(src: &[u8]) -> Option<Bytes> {
        if src.len() <= INLINE {
            let mut buf = [0u8; INLINE];
            buf[..src.len()].copy_from_slice(src);
            return Some(Bytes::Inline {
                len: src.len() as u8,
                buf,
            });
        }
        OwnedBuf::copy_from(src).map(Bytes::Heap)
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Inline { len, buf } => &buf[..usize::from(*len)],
            Bytes::Heap(b) => b.as_slice(),
        }
    }
}

/// Bytes copied out of a Ruby String and checked against `C`: [`RubyText`]
/// is valid UTF-8 with no NUL, [`RubyData`] is valid UTF-8 with NUL permitted
/// (the HTML data family). They are separate types because the contract is
/// the only thing that stops a data-family value from reaching an engine
/// input.
///
/// The view owns its bytes, so nothing Ruby does after the copy - a GC, a
/// mutation of the String, a reallocation - can reach them, and a checked
/// view is a plain `&str` ([`Deref`]) for as long as it lives.
pub struct RubyStr<C> {
    bytes: Bytes,
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
    /// UTF-8 bytes copied, then checked by `check` (which gets the String, for
    /// its cached coderange, and the copy).
    ///
    /// The coderange `check` consults is the String's current one, and the
    /// copy was taken from the String as it is now, with no Ruby in between
    /// ([`copy_bytes`]) - so the cached answer describes the copied bytes.
    fn acquire<P>(
        s: RString,
        check: impl FnOnce(RString, &[u8]) -> Option<P>,
    ) -> Result<Self, Refusal<P>> {
        /* Read in its own encoding: a String in another one is transcoded
         * first, and the copy and the check are of the UTF-8 it became. The
         * transcode is a fresh String on this frame, so the GC keeps it. */
        let s = utf8_strict(s).map_err(Refusal::Encoding)?;
        let bytes = copy_bytes(s).or_oom()?;
        match check(s, bytes.as_slice()) {
            Some(problem) => Err(Refusal::Check(problem)),
            None => Ok(RubyStr {
                bytes,
                contract: core::marker::PhantomData,
            }),
        }
    }
}

impl<C: Checked> Deref for RubyStr<C> {
    type Target = str;

    fn deref(&self) -> &str {
        // SAFETY: the view's own bytes, checked for valid UTF-8 (`Checked`:
        // both contracts include it) when the view was built, and unchanged
        // since - nothing but `Drop` touches them.
        unsafe { core::str::from_utf8_unchecked(self.bytes.as_slice()) }
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

/// The `(ptr, len)` of a String's bytes, as the stable API reads them.
///
/// A String by type, so the bytes are read as one without a check. What is
/// left to the caller is the part no type can say:
///
/// # Safety
/// The returned slice is valid only until Ruby runs - a GC, a mutation or a
/// reallocation of the String can move or free the bytes - so it must not be
/// held across anything that can run Ruby.
#[inline]
unsafe fn borrow<'a>(s: RString) -> &'a [u8] {
    let (s, api) = (s.as_raw(), rb_sys::stable_api::get_default());
    let (ptr, len) = (api.rstring_ptr(s) as *const u8, api.rstring_len(s) as usize);
    if ptr.is_null() || len == 0 {
        return &[];
    }
    core::slice::from_raw_parts(ptr, len)
}

/// Run `f` over the bytes of the live String `s`, borrowed for the call.
///
/// The one scoped borrow this module offers, for a read that needs no copy
/// (the XML encoding sniff). The closure's result must not borrow from the
/// slice, which the signature enforces.
///
/// # Safety
/// `f` must run no Ruby: no allocation of a Ruby object, no method call,
/// nothing that can trigger a GC or let another thread run.
pub(in crate::bridge) unsafe fn with_bytes<R>(s: RString, f: impl FnOnce(&[u8]) -> R) -> R {
    // SAFETY: the caller's contract - nothing runs Ruby while the slice lives.
    f(unsafe { borrow(s) })
}

/// A copy of the live String `s`'s bytes, taken with no Ruby in between; `None`
/// when a heap copy could not be allocated.
fn copy_bytes(s: RString) -> Option<Bytes> {
    // SAFETY: the bytes are read and copied in this call, which runs no Ruby:
    // `Bytes::copy` allocates through libc at most, never through Ruby.
    unsafe { with_bytes(s, Bytes::copy) }
}

/// A live Ruby String's raw bytes, copied into an owned buffer.
pub fn ruby_string_bytes(s: RString) -> Result<OwnedBuf, Error> {
    // SAFETY: as `copy_bytes` - the copy runs no Ruby.
    unsafe { with_bytes(s, OwnedBuf::copy_from) }
        .ok_or_else(|| makiri_error("out of memory reading a Ruby string"))
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
fn encoding_refusal(what: &str, e: Error) -> Error {
    if e.is_kind_of(crate::bridge::ruby::gvl_ruby().exception_encoding_error()) {
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
pub fn ruby_verified_text_opt(in_: Value, what: &str) -> Result<Option<RubyText>, Error> {
    if in_.is_nil() {
        return Ok(None);
    }
    ruby_verified_text(in_, what).map(Some)
}

/// The `namespace_or_nil` argument of the namespaced APIs: `nil` and the empty
/// string ALIKE mean "no namespace", so everything below receives `None` or a
/// non-empty URI and never re-decides.
pub fn namespace_arg(in_: Value, what: &str) -> Result<Option<RubyText>, Error> {
    Ok(ruby_verified_text_opt(in_, what)?.filter(|t| !t.is_empty()))
}

/// Coerce to a String and enforce the strict contract (valid UTF-8, no NUL),
/// naming `what` in the error. The names-and-engine-input path.
pub fn ruby_verified_text(in_: Value, what: &str) -> Result<RubyText, Error> {
    verified_text_raising(in_, what, |m| makiri_error(m))
}

/// [`ruby_verified_text`] for a NAME given to a factory or a setter: the same
/// contract, but a NUL raises `ArgumentError` - the DOM's
/// InvalidCharacterError, which is what every other character a naming rule
/// refuses raises. It raised `Makiri::Error` from here, before any naming rule
/// ran, so a caller translating `ArgumentError` into a DOMException let that
/// one through. Invalid UTF-8 stays `Makiri::Error`: it breaks the String
/// contract every input shares, not a naming rule.
pub fn ruby_verified_name(in_: Value, what: &str) -> Result<RubyText, Error> {
    verified_text_raising(in_, what, |m| crate::bridge::ruby::arg_error(m))
}

/// The one acquisition behind [`ruby_verified_text`] and
/// [`ruby_verified_name`], which differ only in what a NUL raises: `on_nul`
/// is handed the message. Everything else - invalid UTF-8, a copy that ran out
/// of memory - is answered the same way for both.
fn verified_text_raising(
    in_: Value,
    what: &str,
    on_nul: fn(core::fmt::Arguments<'_>) -> Error,
) -> Result<RubyText, Error> {
    use crate::cutf8::TextVerdict;
    let s = name_arg(in_)?;
    RubyText::acquire(s, |s, b| {
        let v = text_check(s, b);
        (v != TextVerdict::Ok).then_some(v)
    })
    .map_err(|r| match r {
        Refusal::Check(TextVerdict::HasNul) => on_nul(format_args!(
            "{what} {}",
            TextVerdict::HasNul.problem().unwrap_or_default()
        )),
        Refusal::Check(v) => text_error(what, v.problem().unwrap_or_default()),
        Refusal::Oom => oom_reading(),
        Refusal::Encoding(e) => encoding_refusal(what, e),
    })
}

/// Coerce to a String and enforce the DATA-family contract: invalid UTF-8 is
/// fatal, an interior NUL is not, so DOM data can hold U+0000 like browsers.
///
/// The same acquisition as [`ruby_verified_text`], with the data check - which
/// lets NUL through - in place of the text one.
pub fn ruby_verified_data(in_: Value, what: &str) -> Result<RubyData, Error> {
    let s = string_arg(in_)?;
    RubyData::acquire(s, |s, b| text_check(s, b).data_problem()).map_err(|r| match r {
        Refusal::Check(problem) => text_error(what, problem),
        Refusal::Oom => oom_reading(),
        Refusal::Encoding(e) => encoding_refusal(what, e),
    })
}

/// Whether Ruby ALREADY knows the String is valid UTF-8.
///
/// This reads the cached classification from the object's flags; it does not
/// scan (a scan would cost as much as running our own validator), so it only
/// wins when Ruby has the answer already. UNKNOWN or BROKEN returns false and
/// the caller validates or sanitises.
///
/// Trusting `Coderange::Valid` is an accepted assumption: Ruby sets it only
/// after a successful scan, and any C extension that rewrote the bytes must go
/// through Ruby's string API to keep the object consistent - one that scribbles
/// over a String without updating it has already broken Ruby's own invariants
/// (String#=~, Encoding, Marshal all read the same cache). The consequence here
/// is sharper than a wrong answer, because the view turns the bytes into a
/// `&str` (see `RubyStr`'s `Deref`); the alternative - scanning every
/// `Coderange::Valid` string ourselves - gives up the optimization on the hot
/// path for a case that means the host is already unsound.
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
    a: RString,
    b: RString,
    max_bytes: usize,
) -> Result<(RubyText, RubyText), &'static str> {
    let a = ruby_try_verified_text(a, max_bytes)?;
    let b = ruby_try_verified_text(b, max_bytes)?;
    Ok((a, b))
}

/// The non-raising form: the checked view, or a static reason on rejection.
/// Nothing is coerced: `sv` is a String by type.
pub fn ruby_try_verified_text(sv: RString, max_bytes: usize) -> Result<RubyText, &'static str> {
    const TOO_LONG: &str = "string exceeds the maximum length";
    /* The cap is on the UTF-8 the String becomes, so it is measured after the
     * transcode: a UTF-16 String is twice its UTF-8 length. Refused before
     * the copy, so an over-long String is not copied whole only to be
     * refused; the check on the copy below stays authoritative. (`acquire`'s
     * own transcode is then a no-op on a String that is UTF-8 already.) */
    let sv = utf8_strict(sv).map_err(|_| NOT_UTF8_CONVERTIBLE)?;
    if sv.len() > max_bytes {
        return Err(TOO_LONG);
    }
    RubyText::acquire(sv, |s, b| {
        if b.len() > max_bytes {
            return Some(TOO_LONG);
        }
        text_check(s, b).reason()
    })
    .map_err(|r| match r {
        Refusal::Check(reason) => reason,
        Refusal::Oom => "could not be copied (out of memory)",
        Refusal::Encoding(_) => NOT_UTF8_CONVERTIBLE,
    })
}
