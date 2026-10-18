//! Ruby String <-> Makiri text.
//!
//! # Every String crosses as a copy
//!
//! A pointer *into* a Ruby String is valid only until Ruby runs: a GC can move
//! or free the backing buffer, and Ruby code can rewrite it - an argument's
//! `#to_s` is arbitrary Ruby, and may change an earlier argument. The C held
//! such pointers and guarded them with a rule nobody could check; the first
//! Rust port held them under `rb_str_locktmp`, with a copy for a String that
//! was already locked, a frozen fast path and an interrupt to re-raise.
//!
//! All of that existed to spare a copy of a few bytes. The Strings that reach
//! the engine through this module are control strings - an XPath expression,
//! a selector, a name, an attribute value - and the one large input, the
//! HTML source, was copied out anyway before the GVL was released. So every
//! String is copied, in one place ([`copy_bytes`]), with no Ruby between the
//! read and the copy. A checked view ([`RubyText`], [`RubyData`]) then owns
//! its bytes: nothing can change them, so its `Deref` to `str` is plain. A
//! short copy stays on the stack ([`Bytes::Inline`]), so the hot paths - a
//! selector, an expression - allocate nothing.
//!
//! What a borrow still buys is a scan that needs no copy at all: the XML
//! reader's encoding sniff reads a prefix of a possibly large String before
//! deciding how to decode it. That is [`with_bytes`], the one scoped borrow,
//! whose closure must run no Ruby.

#![allow(unsafe_code)]
/* Every function takes `VALUE`s its caller holds rooted. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long};
use core::ops::Deref;

use magnus::encoding::Coderange;

use crate::bridge::ruby::makiri_error;
use magnus::rb_sys::AsRawValue;
/* Not magnus's: ours catches a panic before `rb_protect`'s C frame. */
use super::ruby::protect;
use magnus::value::ReprValue;
use magnus::{Error, RString, Value};
use rb_sys::{StableApiDefinition, VALUE};

/// The shared owned buffer.
use crate::cbuf::OwnedBuf;
use crate::falloc::OomOption;

use crate::bridge::ruby::string_of;

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
enum Refusal<P> {
    /// The bytes failed the contract; `P` is the check's own answer.
    Check(P),
    /// The bytes could not be copied.
    Oom,
}

impl<P> crate::falloc::Oom for Refusal<P> {
    #[inline]
    fn oom() -> Self {
        Refusal::Oom
    }
}

impl<C: Checked> RubyStr<C> {
    /// A checked view of `s`: its bytes copied, then checked by `check` (which
    /// gets the String, for its cached coderange, and the copy).
    ///
    /// The coderange `check` consults is the String's current one, and the
    /// copy was taken from the String as it is now, with no Ruby in between
    /// ([`copy_bytes`]) - so the cached answer describes the copied bytes.
    fn acquire<P>(
        s: RString,
        check: impl FnOnce(RString, &[u8]) -> Option<P>,
    ) -> Result<Self, Refusal<P>> {
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
pub(super) unsafe fn with_bytes<R>(s: RString, f: impl FnOnce(&[u8]) -> R) -> R {
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

/* ---- assembling Ruby Strings ---- */

/// Join document-order slices totalling `total` bytes into one UTF-8 String.
///
/// This is the text index's output path, so it writes straight into the fresh
/// String's buffer: one pre-sized allocation and one memcpy run, with no
/// intermediate. The bounds checks are not redundant with the caller's
/// bookkeeping - a wrong `total` would otherwise run past the allocation, so
/// both a long slice and a short sum fail closed.
///
/// # Safety
/// The joined bytes must be valid UTF-8. Text-index slices are, by the
/// text-input contract; bytes that were not would make a wrong String rather
/// than a memory error.
pub unsafe fn ruby_str_from_slices<'a>(
    slices: impl Iterator<Item = &'a [u8]>,
    total: usize,
) -> Result<VALUE, Error> {
    if total > c_long::MAX as usize {
        return Err(makiri_error("text too large to assemble"));
    }
    let str = rb_sys::rb_utf8_str_new(core::ptr::null(), total as c_long);
    /* We just created it and hold the only reference, so writing through the
     * buffer is sound - this is what RSTRING_PTR gives the C. It has room for
     * exactly `total` bytes, and nothing else runs while `out` is held. */
    let dst = rb_sys::stable_api::get_default().rstring_ptr(str) as *mut u8;
    let out = core::slice::from_raw_parts_mut(dst, total);

    let mut off = 0usize;
    for s in slices {
        /* A slice past `total` is refused by the bounds check. */
        let Some(to) = off
            .checked_add(s.len())
            .and_then(|end| out.get_mut(off..end))
        else {
            return Err(makiri_error("text slice length inconsistency"));
        };
        to.copy_from_slice(s);
        off += s.len();
    }
    if off != total {
        /* A short sum would leave the tail of the uninitialised String unwritten. */
        return Err(makiri_error("text slice length inconsistency"));
    }
    Ok(str)
}

/// A UTF-8 String copied from `bytes`.
///
/// The DOM readers hand over a slice the document lends them and want a String
/// of it. Deciding that the bytes are valid UTF-8 is not this layer's job,
/// because only the caller knows where they came from.
///
/// # Safety
/// `bytes` must be valid UTF-8. Everything in a parsed document is, by the
/// text-input contract. Bytes that were not would make a wrong String rather
/// than a memory error - wrong is still wrong.
pub unsafe fn ruby_str_from_utf8(bytes: &[u8]) -> VALUE {
    /* A slice's pointer is never null, even when it is empty. */
    rb_sys::rb_utf8_str_new(bytes.as_ptr() as *const c_char, bytes.len() as c_long)
}

/// A DOM name as a Ruby String: frozen and deduplicated, tagged UTF-8 -
/// Ruby's own `-"div"`, so every read of one name answers the same object and
/// allocates nothing once Ruby has it.
///
/// THE contract for the name readers of both representations: `local_name`,
/// `prefix`, `namespace_uri` and `tag_name` answer through this, and only
/// `#name` (Nokogiri's, which callers may edit) answers a fresh String.
/// Character data - text, attribute values - is never interned.
///
/// Safe to call with any bytes: `rb_enc_interned_str` copies them, and bytes
/// that were not UTF-8 would make a wrong String, never a memory error. Every
/// caller passes a document's name, which is UTF-8 by the text-input contract.
pub fn dom_name_str(bytes: &[u8]) -> Value {
    // SAFETY: a slice's pointer and length, read for the call; the encoding
    // is Ruby's static UTF-8 one. The result is a live String.
    unsafe {
        crate::bridge::ruby::value(rb_sys::rb_enc_interned_str(
            bytes.as_ptr() as *const c_char,
            bytes.len() as c_long,
            rb_sys::rb_utf8_encoding(),
        ))
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
    crate::cutf8::text_verdict(bytes, ruby_str_known_valid_utf8(coderange_str))
}

/// The error for a checked view whose bytes could not be copied.
fn oom_reading() -> Error {
    makiri_error("out of memory reading a Ruby string")
}

/// `Makiri::Error` with "<what> <problem>", the wording the C raised with.
fn text_error(what: &str, problem: &str) -> Error {
    makiri_error(format!("{what} {problem}"))
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
    verified_text_raising(in_, what, makiri_error)
}

/// [`ruby_verified_text`] for a NAME given to a factory or a setter: the same
/// contract, but a NUL raises `ArgumentError` - the DOM's
/// InvalidCharacterError, which is what every other character a naming rule
/// refuses raises. It raised `Makiri::Error` from here, before any naming rule
/// ran, so a caller translating `ArgumentError` into a DOMException let that
/// one through. Invalid UTF-8 stays `Makiri::Error`: it breaks the String
/// contract every input shares, not a naming rule.
pub fn ruby_verified_name(in_: Value, what: &str) -> Result<RubyText, Error> {
    verified_text_raising(in_, what, crate::bridge::ruby::arg_error)
}

/// The one acquisition behind [`ruby_verified_text`] and
/// [`ruby_verified_name`], which differ only in what a NUL raises: `on_nul`
/// is handed the message. Everything else - invalid UTF-8, a copy that ran out
/// of memory - is answered the same way for both.
fn verified_text_raising(
    in_: Value,
    what: &str,
    on_nul: fn(String) -> Error,
) -> Result<RubyText, Error> {
    use crate::cutf8::TextVerdict;
    let s = string_of(in_)?;
    RubyText::acquire(s, |s, b| {
        let v = text_check(s, b);
        (v != TextVerdict::Ok).then_some(v)
    })
    .map_err(|r| match r {
        Refusal::Check(TextVerdict::HasNul) => on_nul(format!(
            "{what} {}",
            TextVerdict::HasNul.problem().unwrap_or_default()
        )),
        Refusal::Check(v) => text_error(what, v.problem().unwrap_or_default()),
        Refusal::Oom => oom_reading(),
    })
}

/// Coerce to a String and enforce the DATA-family contract: invalid UTF-8 is
/// fatal, an interior NUL is not, so DOM data can hold U+0000 like browsers.
///
/// The same acquisition as [`ruby_verified_text`], with the data check - which
/// lets NUL through - in place of the text one.
pub fn ruby_verified_data(in_: Value, what: &str) -> Result<RubyData, Error> {
    let s = string_of(in_)?;
    RubyData::acquire(s, |s, b| text_check(s, b).data_problem()).map_err(|r| match r {
        Refusal::Check(problem) => text_error(what, problem),
        Refusal::Oom => oom_reading(),
    })
}

/* ---- encoding ---- */

/// A Ruby encoding - of a String, or resolved from a name or an `Encoding`.
///
/// Opaque so callers never hold the raw `rb_encoding*`, and the one place the
/// "which encodings are already UTF-8 bytes" rules live. Ruby's encodings are
/// process-lifetime objects, so a value of this type stays valid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Encoding(*mut rb_sys::rb_encoding);

impl Encoding {
    /// The encoding `str` is tagged with.
    pub(super) fn of(str: RString) -> Encoding {
        // SAFETY: a live String, by type; reading its tag runs no Ruby.
        Encoding(unsafe { rb_sys::rb_enc_get(str.as_raw()) })
    }

    pub(super) fn utf8() -> Encoding {
        // SAFETY: Ruby's immutable global encoding.
        Encoding(unsafe { rb_sys::rb_utf8_encoding() })
    }

    /// The encoding Ruby knows by `name`, or None for a name it does not
    /// know - not an error. MAY AUTOLOAD the encoding, a Ruby allocation and so
    /// a GC point: call it with no borrow of a String's bytes held.
    ///
    /// `rb_enc_find` wants a C string and the name is bytes, so the NUL is
    /// added here. A name too long to fit, or holding a NUL, is not one Ruby
    /// knows, so it reads as None rather than being truncated at the NUL.
    pub(super) fn find(name: &[u8]) -> Option<Encoding> {
        let mut buf = [0u8; 64];
        if name.is_empty() || name.len() >= buf.len() || name.contains(&0) {
            return None;
        }
        buf[..name.len()].copy_from_slice(name);
        let c = core::ffi::CStr::from_bytes_with_nul(&buf[..name.len() + 1]).ok()?;
        // SAFETY: a NUL-terminated name; the lookup raises nothing.
        let enc = unsafe { rb_sys::rb_enc_find(c.as_ptr()) };
        (!enc.is_null()).then_some(Encoding(enc))
    }

    /// The raw encoding, for a C call in the bridge that takes one.
    pub(super) fn as_raw(self) -> *mut rb_sys::rb_encoding {
        self.0
    }

    pub(super) fn is_usascii(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_usascii_encoding() }
    }

    pub(super) fn is_ascii8bit(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_ascii8bit_encoding() }
    }

    /// UTF-8 or US-ASCII: text in it is already UTF-8, character for
    /// character. What a serialized String needs no transcoding into.
    fn is_utf8_compatible(self) -> bool {
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
    pub(super) fn reads_as_utf8_bytes(self) -> bool {
        self.is_utf8_compatible() || self.is_ascii8bit()
    }

    /// `str` transcoded to this encoding, a character it cannot represent
    /// becoming a hex character reference. A transcoding failure is returned,
    /// not raised.
    pub fn encode_charref(self, str: Value) -> Result<Value, Error> {
        const UNDEF_HEX_CHARREF: c_int =
            rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_HEX_CHARREF as c_int;
        // SAFETY: `str` is a live String and `self` a live encoding; `protect`
        // turns a raise into `Err`.
        let raw = protect(|| unsafe {
            rb_sys::rb_str_encode(
                str.as_raw(),
                rb_sys::rb_enc_from_encoding(self.0),
                UNDEF_HEX_CHARREF,
                rb_sys::Qnil as VALUE,
            )
        })?;
        // SAFETY: `rb_str_encode` returns a live String value.
        Ok(unsafe { crate::bridge::ruby::value(raw) })
    }
}

/// The encoding `v` names, or the error Ruby's own lookup raises: `ArgumentError`
/// for an unknown name, `TypeError` for something that is neither a String nor
/// an Encoding.
pub fn to_encoding(v: Value) -> Result<Encoding, Error> {
    let mut enc: *mut rb_sys::rb_encoding = core::ptr::null_mut();
    // SAFETY: `v` is a live value; `protect` turns the raise into `Err`.
    protect(|| unsafe {
        enc = rb_sys::rb_to_encoding(v.as_raw());
        rb_sys::Qnil as VALUE
    })?;
    Ok(Encoding(enc))
}

/// A UTF-8 String for `str`, honouring its declared encoding so the content
/// survives.
///
///  - UTF-8 / US-ASCII / ASCII-8BIT: returned unchanged. These are already
///    UTF-8 bytes, or deliberately raw ones, and the native parser does the
///    WHATWG invalid-byte replacement for them. The common case costs one
///    encoding comparison - no transcode, no copy.
///  - anything else (Shift_JIS, EUC-JP, ISO-8859-1, Windows-1252, ...):
///    transcoded with invalid/undef -> U+FFFD, so the text becomes the right
///    characters instead of being read as raw UTF-8 and mangled. Only
///    non-UTF-8 input pays for this.
///
/// `rb_str_encode` RAISES when Ruby has no converter at all (UTF-7,
/// ISO-2022-JP-2 to UTF-8), so it is only ever called under [`protect`] - see
/// [`ruby_to_utf8_value`]. Called bare, that raise would `longjmp` over the
/// Rust frames above it.
unsafe fn ruby_to_utf8(str: RString) -> VALUE {
    if Encoding::of(str).reads_as_utf8_bytes() {
        return str.as_raw();
    }
    const REPLACE: c_int = rb_sys::ruby_econv_flag_type::RUBY_ECONV_INVALID_REPLACE as c_int
        | rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_REPLACE as c_int;
    rb_sys::rb_str_encode(
        str.as_raw(),
        rb_sys::rb_enc_from_encoding(Encoding::utf8().0),
        REPLACE,
        rb_sys::Qnil as VALUE,
    )
}

/// [`ruby_to_utf8`] as a safe call: `s` is a String by type, and so is the
/// result. An encoding Ruby cannot convert to UTF-8 comes back as its
/// `Encoding::ConverterNotFoundError`, returned rather than raised.
pub fn ruby_to_utf8_value(s: RString) -> Result<RString, Error> {
    // SAFETY: `s` is a live String, and `protect` turns the raise into `Err`.
    let raw = protect(|| unsafe { ruby_to_utf8(s) })?;
    // SAFETY: `rb_str_encode` returns a live value; checked to be a String.
    RString::from_value(unsafe { crate::bridge::ruby::value(raw) })
        .ok_or_else(|| crate::bridge::ruby::internal_error("transcoding returned a non-String"))
}

/// A Ruby String as HTML parser input, under the text-input contract: its
/// encoding honoured ([`ruby_to_utf8_value`]), its bytes copied out, and
/// whether they are already known to be valid UTF-8, so the parser can skip
/// its sanitisation.
///
/// The one place that turns a Ruby String into bytes an HTML parser reads.
/// Both HTML entry points - a document parse and a fragment parse - take their
/// input through it, so the engine below never sees a `VALUE`. The copy is
/// what a document parse needs anyway, to read the source with the GVL
/// released.
pub struct HtmlSource {
    bytes: OwnedBuf,
    known_valid: bool,
}

impl HtmlSource {
    /// `s` is a String by type: the caller has coerced it.
    pub fn from_ruby(s: RString) -> Result<HtmlSource, Error> {
        let src = ruby_to_utf8_value(s)?;
        /* A transcode replaced every invalid or unmappable byte, so its result
         * is valid UTF-8 whatever its coderange says. */
        let transcoded = src.as_raw() != s.as_raw();
        let known_valid = transcoded || ruby_str_known_valid_utf8(src);
        let bytes = ruby_string_bytes(src)?;
        Ok(HtmlSource { bytes, known_valid })
    }

    /// Whether the bytes are valid UTF-8 already - never a scan, only what
    /// Ruby (or the transcode) has established.
    pub fn known_valid(&self) -> bool {
        self.known_valid
    }

    /// The bytes.
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// The bytes, owned - for a parse that runs with the GVL released.
    pub fn into_bytes(self) -> OwnedBuf {
        self.bytes
    }
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
fn ruby_str_known_valid_utf8(str: RString) -> bool {
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
    RubyText::acquire(sv, |s, b| {
        if b.len() > max_bytes {
            return Some("string exceeds the maximum length");
        }
        text_check(s, b).reason()
    })
    .map_err(|r| match r {
        Refusal::Check(reason) => reason,
        Refusal::Oom => "could not be copied (out of memory)",
    })
}
