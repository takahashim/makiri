//! Ruby String <-> Makiri text.
//!
//! # The borrow rule this file exists to hold
//!
//! Several functions hand back a pointer *into* a Ruby String. That borrow is
//! only valid until Ruby is allowed to run: a GC can move or free the backing
//! buffer. So the verdict check in [`text_check`] is **allocation-free by
//! design**, and every caller relies on that - it runs between a caller taking
//! the pointer and using it, so it must not be a GC point.
//!
//! (The C's history is the warning: each caller used to build a throwaway Ruby
//! String just to read its coderange, which put a Ruby allocation inside every
//! borrow, and opened a GC window under every *other* borrow already held at a
//! multi-borrow call site.)
//!
//! Rust does not enforce this for us - `RString::as_slice` is `unsafe` for
//! exactly this reason, and its lifetime is tied to nothing. What it does give
//! is a place to state the rule once, which is here.

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

use crate::bridge::ruby::string_of;

/* ---- the borrowed-text layouts ----
 *
 * Three borrowed-text types with ONE layout. The distinction is the contract,
 * not the shape: `text` has been checked for valid UTF-8 *and* no NUL, `data` for UTF-8 only (the HTML
 * data family may hold U+0000, like browsers), and `bytes` for nothing at all
 * (HTML parsing decodes leniently). Keeping them apart is what makes a name or
 * engine string that took the data path a type error rather than a silent one,
 * so they stay three types here: [`RubyText`] / [`RubyData`] / [`RubyBytes`],
 * one guard type with the contract as its parameter. */

/// The contract a [`RubyStr`] was checked against. Uninhabited: types only.
pub enum TextContract {}
/// See [`TextContract`].
pub enum DataContract {}
/// See [`TextContract`].
pub enum BytesContract {}

mod sealed {
    pub trait Checked {}
    impl Checked for super::TextContract {}
    impl Checked for super::DataContract {}
}
use sealed::Checked;

/// Bytes borrowed from a Ruby String, together with the String that owns them.
///
/// The parameter records what was checked: [`RubyText`] is valid UTF-8 with no
/// NUL, [`RubyData`] is valid UTF-8 with NUL permitted (the HTML data family),
/// and [`RubyBytes`] is unchecked (HTML parsing decodes leniently). They are
/// separate types because the contract is the only thing that stops a
/// data-family value from reaching an engine input.
///
/// `Drop` is the keep-alive. It reads `value`, so the String stays visible to
/// the conservative stack scan until the guard goes out of scope - the C's
/// `RB_GC_GUARD` at the end of the borrow, without each call site having to
/// remember it. For a non-String argument that String is the coerced one, which
/// nothing else holds. Hence: not `Copy`, kept on the stack (never in a heap
/// container, which the GC does not scan), and read through `&self`.
///
/// Anchoring keeps the String alive and in place; it does not stop Ruby code
/// from mutating it. A CHECKED view ([`RubyText`], [`RubyData`]) is therefore
/// held immutable for its whole life ([`RubyStr::acquire`]): a mutator converts
/// its arguments one after another, and the second one's `#to_s` is arbitrary
/// Ruby that could rewrite the first - putting a NUL into a name that had
/// passed the check, or reallocating it so the view read freed memory. That is
/// what makes a checked view a plain `&str` ([`Deref`]), like a `MutexGuard`.
/// The unchecked [`RubyBytes`] (the parse source) is not held; its bytes stay
/// behind an `unsafe` accessor, and it is copied before the GVL is released.
pub struct RubyStr<C> {
    value: VALUE,
    ptr: *const u8,
    len: usize,
    /// Whether this view took the String's temporary lock, and so releases it.
    owns_lock: bool,
    /// The view's own copy of the bytes, when the String was already locked by
    /// someone else (see [`RubyStr::acquire`]); `ptr` then points into it.
    _copy: Option<OwnedBuf>,
    contract: core::marker::PhantomData<C>,
}

pub type RubyText = RubyStr<TextContract>;
pub type RubyData = RubyStr<DataContract>;
pub type RubyBytes = RubyStr<BytesContract>;

impl<C> RubyStr<C> {
    /// # Safety
    /// `ptr`/`len` must be the bytes of the String `value`, checked against `C`.
    unsafe fn from_raw_parts(value: VALUE, ptr: *const u8, len: usize) -> Self {
        Self {
            value,
            ptr,
            len,
            owns_lock: false,
            _copy: None,
            contract: core::marker::PhantomData,
        }
    }
}

/// Why [`RubyStr::acquire`] gave no view.
enum Refusal<P> {
    /// The held bytes failed the contract; `P` is the check's own answer.
    Check(P),
    /// The String was locked by someone else and its bytes could not be copied.
    Oom,
}

impl<C: Checked> RubyStr<C> {
    /// A checked view of `s`: its bytes held immutable until the view drops,
    /// and only THEN checked, by `check` (which gets the String, for its cached
    /// coderange, and the held bytes).
    ///
    /// Held normally means the String's temporary lock: Ruby raises "can't
    /// modify string; temporarily locked" on any change, reallocation included
    /// (`IO#write` holds its buffer the same way).
    ///
    /// `rb_str_locktmp` raises when the String is already locked - by another
    /// view of the same argument passed twice, or by an IO. That lock is not
    /// this view's to rely on: its holder releases it when IT is done, which
    /// can be while this view still lives (an IO on another thread finishing
    /// during a later argument's `#to_s`, or the earlier view dropping first),
    /// and after that Ruby may rewrite or free the bytes under a `&str` this
    /// view handed out. So the view copies the bytes instead and reads its own
    /// copy - which keeps the answer (the same argument twice still works) and
    /// the safe `Deref` sound, at an allocation only in that rare case.
    ///
    /// The order is the point. Building the "already locked" error runs Ruby
    /// (the exception's `initialize`, an interrupt check, another thread), so
    /// bytes borrowed or checked BEFORE the lock attempt may be gone or changed
    /// by the time they are copied. They are borrowed only after it, when
    /// nothing below runs Ruby until they are locked or copied, and the check
    /// reads exactly what the view will hand out.
    fn acquire<P>(
        s: RString,
        check: impl FnOnce(RString, &[u8]) -> Option<P>,
    ) -> Result<Self, Refusal<P>> {
        let value = s.as_raw();
        // SAFETY: `value` is the live String `s`; `protect` turns the
        // already-locked raise into `Err`.
        let owns_lock = protect(|| unsafe { rb_sys::rb_str_locktmp(value) }).is_ok();
        // SAFETY: borrowed after the lock attempt and whatever Ruby it ran;
        // nothing from here runs Ruby before the bytes are held (locked, or
        // copied into the view's own buffer).
        let (_, ptr, len) = unsafe { borrow(s) };
        // SAFETY: the bytes of `value`, checked below before the view is
        // returned.
        let mut view = unsafe { Self::from_raw_parts(value, ptr, len) };
        view.owns_lock = owns_lock;
        if !owns_lock {
            // SAFETY: as above - no Ruby since the borrow.
            let copy = OwnedBuf::copy_from(unsafe { bytes_at(ptr, len) }).ok_or(Refusal::Oom)?;
            /* The copy's heap storage does not move with the view. */
            view.ptr = copy.as_slice().as_ptr();
            view._copy = Some(copy);
        }
        /* The coderange `check` may consult is the String's current one, and
         * the String has not changed since the bytes were taken. */
        // SAFETY: the held bytes, live for the view's life.
        match check(s, unsafe { bytes_at(view.ptr, view.len) }) {
            /* Dropping the view releases a lock it took. */
            Some(problem) => Err(Refusal::Check(problem)),
            None => Ok(view),
        }
    }
}

impl<C: Checked> Deref for RubyStr<C> {
    type Target = str;

    fn deref(&self) -> &str {
        // SAFETY: `ptr`/`len` are either the bytes of the String this view
        // holds locked - anchored (so alive and unmoved) and unmodifiable until
        // `Drop` unlocks it, which cannot run while `&self` is borrowed - or
        // the view's own copy, which it owns for as long. Nobody else unlocks a
        // lock this view took. Either way they were checked for valid UTF-8
        // (`Checked`: both contracts include it) when the view was built.
        unsafe { core::str::from_utf8_unchecked(bytes_at(self.ptr, self.len)) }
    }
}

impl RubyText {
    /// The text as an engine input, borrowed for the guard's lifetime.
    pub(crate) fn text(&self) -> crate::text::VerifiedText<'_> {
        /* The bridge checked the text contract when it built `self`. */
        crate::text::VerifiedText::from_checked(self)
    }
}

impl RubyBytes {
    /// The bytes.
    ///
    /// # Safety
    /// No Ruby code may run, and so mutate the String, while the slice is used.
    pub(crate) unsafe fn bytes(&self) -> &[u8] {
        bytes_at(self.ptr, self.len)
    }

    /// The bytes copied into an owned buffer, usable while the GVL is released
    /// or across a Ruby allocation. An allocation failure is an `Err` - a
    /// caller that needed the bytes must not carry on without them.
    pub fn to_owned_buf(&self) -> Result<OwnedBuf, Error> {
        // SAFETY: the copy runs no Ruby code, and the guard anchors the String.
        OwnedBuf::copy_from(unsafe { self.bytes() })
            .ok_or_else(|| makiri_error("out of memory reading a Ruby string"))
    }
}

impl<C> Drop for RubyStr<C> {
    fn drop(&mut self) {
        if self.owns_lock {
            let v = self.value;
            // SAFETY: the String this view locked, still alive (the view
            // anchors it). Only the locker unlocks, so it is still locked.
            let _ = protect(|| unsafe { rb_sys::rb_str_unlocktmp(v) });
        }
        core::hint::black_box(self.value);
    }
}

/// `len` bytes at `ptr`, or the empty slice when there are none (whatever the
/// pointer).
///
/// # Safety
/// When `len > 0`, `ptr` must point to `len` bytes that stay live and unchanged
/// for `'a`.
#[inline]
unsafe fn bytes_at<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        return &[];
    }
    core::slice::from_raw_parts(ptr, len)
}

/// What the strict-text check found.
///
/// The enum itself lives in [`crate::cutf8`] beside the pure [`text_verdict`];
/// re-exported here so the bridge's callers keep naming it from this module.
pub use crate::cutf8::TextVerdict;

/// The `value` + `(ptr, len)` of a String, taken together so the borrow and its
/// anchor cannot be separated by accident.
///
/// A String by type, so the bytes are read as one without a check. What is
/// left to the caller is the part no type can say:
///
/// # Safety
/// The returned pointer is valid only until Ruby runs - a GC, a mutation or a
/// reallocation of the String can move or free the bytes - so it must not be
/// held across anything that can run Ruby or allocate.
#[inline]
unsafe fn borrow(s: RString) -> (VALUE, *const u8, usize) {
    let (s, api) = (s.as_raw(), rb_sys::stable_api::get_default());
    (
        s,
        api.rstring_ptr(s) as *const u8,
        api.rstring_len(s) as usize,
    )
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

/* ---- the strict text contract ---- */

/// Check `bytes` against the strict contract, returning the specific
/// violation so each caller can map it to its own error surface (`Makiri::Error`,
/// `XML::SyntaxError`, or a reason string).
///
/// It resolves the String's cached coderange into an ordinary value, then
/// delegates the actual check to the pure [`crate::cutf8::text_verdict`]; the
/// logic is Ruby-free and testable under the always-compiled core.
///
/// `coderange_str` is consulted only for its CACHED coderange, which never
/// scans and never allocates - so a caller may pass bytes it borrowed from a
/// String and still hold them afterwards. Allocation-free - see the module
/// docs.
pub fn text_check(coderange_str: RString, bytes: &[u8]) -> TextVerdict {
    /* The cached coderange reads flags; it never scans and never allocates. */
    crate::cutf8::text_verdict(bytes, ruby_str_known_valid_utf8(coderange_str))
}

/// The error for a checked view whose bytes could not be copied - see
/// [`RubyStr::acquire`].
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

/// Coerce to a String and enforce the strict contract (valid UTF-8, no NUL),
/// naming `what` in the error. The names-and-engine-input path.
pub fn ruby_verified_text(in_: Value, what: &str) -> Result<RubyText, Error> {
    let s = string_of(in_)?;
    RubyText::acquire(s, |s, b| text_check(s, b).problem()).map_err(|r| match r {
        Refusal::Check(problem) => text_error(what, problem),
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

/// A borrowed raw byte view. Deliberately enforces nothing: HTML parsing
/// consumes raw bytes and decodes invalid UTF-8 leniently, like a browser.
///
/// `s` is a String by type, so there is nothing to convert, and nothing here
/// can raise. The view's bytes are [`borrow`]'s: valid only until Ruby runs.
pub unsafe fn ruby_bytes_view(s: RString) -> RubyBytes {
    let (value, ptr, len) = borrow(s);
    RubyBytes::from_raw_parts(value, ptr, len)
}

/// A live Ruby String's raw bytes, copied into an owned buffer.
pub fn ruby_string_bytes(s: RString) -> Result<OwnedBuf, Error> {
    // SAFETY: `s` is a live Ruby String; the view anchors it for the copy.
    unsafe { ruby_bytes_view(s) }.to_owned_buf()
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
    fn of(str: RString) -> Encoding {
        // SAFETY: a live String, by type; reading its tag runs no Ruby.
        Encoding(unsafe { rb_sys::rb_enc_get(str.as_raw()) })
    }

    fn utf8() -> Encoding {
        // SAFETY: Ruby's immutable global encoding.
        Encoding(unsafe { rb_sys::rb_utf8_encoding() })
    }

    fn is_usascii(self) -> bool {
        // SAFETY: Ruby's immutable global encoding.
        self.0 == unsafe { rb_sys::rb_usascii_encoding() }
    }

    fn is_ascii8bit(self) -> bool {
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

    /// Whether HTML input in this encoding is parsed as it is: UTF-8, US-ASCII,
    /// or ASCII-8BIT - deliberately raw bytes, which the parser decodes
    /// leniently. Anything else is transcoded to UTF-8 first.
    fn parses_as_is(self) -> bool {
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
    if Encoding::of(str).parses_as_is() {
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
        .ok_or_else(|| makiri_error("transcoding returned a non-String"))
}

/// A Ruby String as HTML parser input, under the text-input contract: its
/// encoding honoured ([`ruby_to_utf8_value`]), and whether the bytes are
/// already known to be valid UTF-8, so the parser can skip its sanitisation.
///
/// The one place that turns a Ruby String into bytes a Lexbor parser reads.
/// Both HTML entry points - a document parse and a fragment parse - take their
/// input through it, so the engine below never sees a `VALUE`.
///
/// The bytes are borrowed from the String, or from the transcoded copy, which
/// the guard inside keeps alive: like every [`RubyStr`], this lives on the
/// stack and is read only while no Ruby code runs.
pub struct HtmlSource {
    view: RubyBytes,
    known_valid: bool,
}

impl HtmlSource {
    /// `s` is a String by type: the caller has coerced it.
    pub fn from_ruby(s: RString) -> Result<HtmlSource, Error> {
        let src = ruby_to_utf8_value(s)?;
        /* A transcode replaced every invalid or unmappable byte, so its result
         * is valid UTF-8 whatever its coderange says. */
        let transcoded = src.as_raw() != s.as_raw();
        // SAFETY: `src` is a live String; the view anchors it.
        let known_valid = transcoded || ruby_str_known_valid_utf8(src);
        // SAFETY: as above.
        let view = unsafe { ruby_bytes_view(src) };
        Ok(HtmlSource { view, known_valid })
    }

    /// Whether the bytes are valid UTF-8 already - never a scan, only what
    /// Ruby (or the transcode) has established.
    pub fn known_valid(&self) -> bool {
        self.known_valid
    }

    /// The bytes.
    ///
    /// # Safety
    /// No Ruby code may run, and so move or mutate the String, while the slice
    /// is used.
    pub unsafe fn bytes(&self) -> &[u8] {
        self.view.bytes()
    }

    /// The bytes copied out, for a parse that runs with the GVL released.
    pub fn to_owned_bytes(&self) -> Result<OwnedBuf, Error> {
        self.view.to_owned_buf()
    }
}

/// Whether Ruby ALREADY knows the String is valid UTF-8.
///
/// This reads the cached classification from the object's flags; it does not
/// scan (a scan would cost as much as running our own validator), so it only
/// wins when Ruby has the answer already. UNKNOWN or BROKEN returns false and
/// the caller validates or sanitises.
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

/// [`ruby_try_verified_text`] for two Strings at once (a `{prefix => uri}` pair),
/// as a safe call: both are live Strings.
pub fn ruby_try_verified_text_pair(
    a: RString,
    b: RString,
    max_bytes: usize,
) -> Result<(RubyText, RubyText), &'static str> {
    Ok((
        ruby_try_verified_text(a, max_bytes)?,
        ruby_try_verified_text(b, max_bytes)?,
    ))
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
