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

use core::ffi::{c_char, c_int, c_long, CStr};

use magnus::encoding::Coderange;

use crate::bridge::ruby::makiri_error;
use magnus::rb_sys::{AsRawValue, FromRawValue};
/* Not magnus's: ours catches a panic before `rb_protect`'s C frame. */
use super::ruby::protect;
use magnus::value::ReprValue;
use magnus::{Error, RString, Value};
use rb_sys::{StableApiDefinition, VALUE};

/// The shared owned buffer.
use crate::cbuf::OwnedBuf;
/// The UNANCHORED, NUL-permitting slice from `crate::text` - a different type
/// from the Ruby-anchored [`RubyText`] below, despite the family resemblance.
/// Text-index slices and Lexbor-interned names reach Ruby through it.
pub use crate::text::BorrowedText;

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

/// Bytes borrowed from a Ruby String, together with the String that owns them.
///
/// The parameter records what was checked: [`RubyText`] is valid UTF-8 with no
/// NUL (`ptr` is NUL-terminated, so it also works as a C string), [`RubyData`]
/// is valid UTF-8 with NUL permitted (the HTML data family), and [`RubyBytes`]
/// is unchecked (HTML parsing decodes leniently). They are separate types
/// because the contract is the only thing that stops a data-family value from
/// reaching an engine input.
///
/// `Drop` is the keep-alive. It reads `value`, so the String stays visible to
/// the conservative stack scan until the guard goes out of scope - the C's
/// `RB_GC_GUARD` at the end of the borrow, without each call site having to
/// remember it. For a non-String argument that String is the coerced one, which
/// nothing else holds. Hence: not `Copy`, kept on the stack (never in a heap
/// container, which the GC does not scan), and read through `&self`.
///
/// Anchoring keeps the String alive and in place; it does not stop Ruby code
/// from mutating it. A CHECKED view ([`RubyText`], [`RubyData`]) therefore also
/// holds the String's temporary lock ([`RubyStr::locked`]) for its life: a
/// mutator converts its arguments one after another, and the second one's
/// `#to_s` is arbitrary Ruby that could rewrite the first - putting a NUL into
/// a name that had passed the check, or reallocating it so the view read freed
/// memory. Locked, that `#to_s` raises instead. The unchecked [`RubyBytes`]
/// (the parse source) is not locked; it is copied before the GVL is released.
pub struct RubyStr<C> {
    value: VALUE,
    ptr: *const c_char,
    len: usize,
    /// Whether this view took the String's temporary lock, and so releases it.
    owns_lock: bool,
    contract: core::marker::PhantomData<C>,
}

pub type RubyText = RubyStr<TextContract>;
pub type RubyData = RubyStr<DataContract>;
pub type RubyBytes = RubyStr<BytesContract>;

impl<C> RubyStr<C> {
    /// # Safety
    /// `ptr`/`len` must be the bytes of the String `value`, checked against `C`.
    pub(crate) unsafe fn from_raw_parts(value: VALUE, ptr: *const c_char, len: usize) -> Self {
        Self {
            value,
            ptr,
            len,
            owns_lock: false,
            contract: core::marker::PhantomData,
        }
    }

    /// The view, with its String held immutable until the view drops: Ruby
    /// raises "can't modify string; temporarily locked" on any change,
    /// reallocation included (`IO#write` holds its buffer the same way).
    ///
    /// `rb_str_locktmp` raises when the String is already locked - by another
    /// view of the same argument passed twice, or by an IO - and the holder of
    /// that lock releases it; this view then only anchors. Taken right after
    /// the borrow, with no Ruby run in between.
    pub(crate) fn locked(mut self) -> Self {
        if !self.ptr.is_null() {
            let v = self.value;
            // SAFETY: `v` is the live String this view borrows; `protect`
            // turns the already-locked raise into `Err`.
            self.owns_lock = protect(|| unsafe { rb_sys::rb_str_locktmp(v) }).is_ok();
        }
        self
    }

    /// No String at all: a null pointer, which Lexbor and the engine read as an
    /// omitted argument.
    pub(crate) fn absent() -> Self {
        Self {
            value: rb_sys::Qnil as VALUE,
            ptr: core::ptr::null(),
            len: 0,
            owns_lock: false,
            contract: core::marker::PhantomData,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The bytes, or an empty slice when absent.
    ///
    /// # Safety
    /// No Ruby code may run, and so mutate the String, while the slice is used.
    pub(crate) unsafe fn bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.ptr as *const u8, self.len)
    }
}

impl RubyText {
    /// The bytes as an engine input, borrowed for the guard's lifetime.
    ///
    /// Safe in the lifetime sense: the token cannot outlive `self`, so the
    /// anchor keeps the bytes alive for every use. The engine is Ruby-free, so
    /// nothing runs that could move them while the token is held.
    pub(crate) fn as_verified(&self) -> crate::text::VerifiedText<'_> {
        // SAFETY: the bridge checked the text contract when it built `self`,
        // and the returned lifetime is the borrow of the guard.
        unsafe { crate::text::VerifiedText::from_raw_parts(self.ptr, self.len) }
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

/// What the strict-text check found.
///
/// The enum itself lives in [`crate::cutf8`] beside the pure [`text_verdict`];
/// re-exported here so the bridge's callers keep naming it from this module.
pub use crate::cutf8::TextVerdict;

/// The `value` + `(ptr, len)` of a String, taken together so the borrow and its
/// anchor cannot be separated by accident.
///
/// # Safety
/// `s` must be a `T_STRING`. The returned pointer is valid only until Ruby runs.
#[inline]
unsafe fn borrow(s: VALUE) -> (VALUE, *const c_char, usize) {
    /* The contract says `T_STRING`, so this reads it as one - what magnus's
     * checked conversion did after a type test this function does not need. */
    let api = rb_sys::stable_api::get_default();
    (s, api.rstring_ptr(s), api.rstring_len(s) as usize)
}

/* ---- assembling Ruby Strings ---- */

/// Join `n` document-order slices totalling `total` bytes into one UTF-8 String.
///
/// This is the text index's output path, so it writes straight into the fresh
/// String's buffer: one pre-sized allocation and one memcpy run, with no
/// intermediate. The bounds checks are not redundant with the caller's
/// bookkeeping - a wrong `total` would otherwise run past the allocation, so
/// both a long slice and a short sum fail closed.
pub unsafe fn ruby_str_from_slices(slices: &[BorrowedText], total: usize) -> Result<VALUE, Error> {
    if total > c_long::MAX as usize {
        return Err(makiri_error("text too large to assemble"));
    }
    let str = rb_sys::rb_utf8_str_new(core::ptr::null(), total as c_long);
    /* We just created it and hold the only reference, so writing through the
     * buffer is sound - this is what RSTRING_PTR gives the C. */
    let dst = rb_sys::stable_api::get_default().rstring_ptr(str) as *mut u8;

    let mut off = 0usize;
    for s in slices {
        if s.is_empty() {
            continue;
        }
        if s.len() > total - off {
            /* off <= total holds, so the subtraction cannot underflow. */
            return Err(makiri_error("text slice length inconsistency"));
        }
        core::ptr::copy_nonoverlapping(s.as_ptr() as *const u8, dst.add(off), s.len());
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
/// of it. Minting the [`BorrowedText`] for that is this layer's job; deciding
/// that the bytes satisfy its contract is not, because only the caller knows
/// where they came from.
///
/// # Safety
/// `bytes` must be valid UTF-8. Everything in a parsed document is, by the
/// text-input contract. Bytes that were not would make a wrong String rather
/// than a memory error - wrong is still wrong.
pub unsafe fn ruby_str_from_utf8(bytes: &[u8]) -> VALUE {
    ruby_str_from_borrowed(BorrowedText::from_raw_parts(
        bytes.as_ptr() as *const c_char,
        bytes.len(),
    ))
}

/// A UTF-8 String copied from a borrowed slice. NULL is the "absent" sentinel
/// and yields `""` whatever `len` says, so the sentinel is never dereferenced.
unsafe fn ruby_str_from_borrowed(text: BorrowedText) -> VALUE {
    if text.is_absent() {
        return rb_sys::rb_utf8_str_new(c"".as_ptr(), 0);
    }
    rb_sys::rb_utf8_str_new(text.as_ptr(), text.len() as c_long)
}

/* ---- the strict text contract ---- */

/// Check `[ptr, len)` against the strict contract, returning the specific
/// violation so each caller can map it to its own error surface (`Makiri::Error`,
/// `XML::SyntaxError`, or a reason string).
///
/// A thin `unsafe` boundary: it resolves the raw pointer and the String's cached
/// coderange into ordinary values, then delegates the actual check to the pure
/// [`crate::cutf8::text_verdict`]. This is the only `unsafe` in the path; the
/// logic is Ruby-free and testable under the always-compiled core.
///
/// `coderange_str` is consulted only for its CACHED coderange, which never
/// scans and never allocates.
///
/// Allocation-free - see the module docs.
///
/// # Safety
/// `ptr` must be readable for `len` bytes (or null), and `coderange_str` must be
/// a valid `T_STRING`. Both borrows must not be held across a Ruby allocation.
pub unsafe fn text_check(coderange_str: VALUE, ptr: *const c_char, len: usize) -> TextVerdict {
    let bytes = if ptr.is_null() || len == 0 {
        &[][..]
    } else {
        core::slice::from_raw_parts(ptr as *const u8, len)
    };
    /* The cached coderange reads flags; it never scans and never allocates. */
    crate::cutf8::text_verdict(bytes, ruby_str_known_valid_utf8(coderange_str))
}

/// Enforce the strict contract (valid UTF-8, no NUL) on the String `str`,
/// naming `what` in the `Makiri::Error`.
pub fn verify_text(str: RString, what: &CStr) -> Result<(), Error> {
    let str = str.as_raw();
    // SAFETY: `str` is a live String, and the borrow ends with the check -
    // before anything below can allocate.
    let verdict = unsafe {
        let (_, ptr, len) = borrow(str);
        text_check(str, ptr, len)
    };
    let Some(problem) = verdict.problem() else {
        return Ok(());
    };
    /* The borrow is not used past the check, so building the message may
     * allocate. */
    Err(text_error(what, problem))
}

/// `Makiri::Error` with "<what> <problem>", the wording the C raised with.
fn text_error(what: &CStr, problem: &str) -> Error {
    let what = what.to_string_lossy();
    makiri_error(format!("{what} {problem}"))
}

/// [`ruby_verified_text`] for an optional argument: `nil` is `None`.
pub fn ruby_verified_text_opt(in_: Value, what: &CStr) -> Result<Option<RubyText>, Error> {
    if in_.is_nil() {
        return Ok(None);
    }
    ruby_verified_text(in_, what).map(Some)
}

/// Coerce to a String and enforce the strict contract (valid UTF-8, no NUL),
/// naming `what` in the error. The names-and-engine-input path.
pub fn ruby_verified_text(in_: Value, what: &CStr) -> Result<RubyText, Error> {
    let s = string_of(in_)?;
    verify_text(s, what)?;
    // SAFETY: `s` is a live String that has just passed the text contract; the
    // view anchors it.
    unsafe {
        let (value, ptr, len) = borrow(s.as_raw());
        Ok(RubyText::from_raw_parts(value, ptr, len).locked())
    }
}

/// Coerce to a String and enforce the DATA-family contract: invalid UTF-8 is
/// fatal, an interior NUL is not, so DOM data can hold U+0000 like browsers.
///
/// `verify_text` is not reused because it rejects NUL. The check is
/// allocation-free, so the borrow taken before it is not held across a GC point.
pub fn ruby_verified_data(in_: Value, what: &CStr) -> Result<RubyData, Error> {
    let s = string_of(in_)?.as_raw();
    // SAFETY: `s` is a live String; the check reads its bytes without
    // allocating, and the view anchors it.
    unsafe {
        let (value, ptr, len) = borrow(s);
        if let Some(problem) = text_check(s, ptr, len).data_problem() {
            return Err(text_error(what, problem));
        }
        Ok(RubyData::from_raw_parts(value, ptr, len).locked())
    }
}

/// A borrowed raw byte view. Deliberately enforces nothing: HTML parsing
/// consumes raw bytes and decodes invalid UTF-8 leniently, like a browser.
///
/// `s` must already be a String - every caller passes one it has just coerced,
/// transcoded or decoded - so there is nothing to convert, and nothing here can
/// raise.
pub unsafe fn ruby_bytes_view(s: VALUE) -> RubyBytes {
    let (value, ptr, len) = borrow(s);
    RubyBytes::from_raw_parts(value, ptr, len)
}

impl<C> RubyStr<C> {
    /// The bytes copied into an owned buffer, usable while the GVL is released
    /// or across a Ruby allocation. An allocation failure is an `Err` - a
    /// caller that needed the bytes must not carry on without them.
    pub fn to_owned_buf(&self) -> Result<OwnedBuf, Error> {
        // SAFETY: the copy runs no Ruby code, and the guard anchors the String.
        OwnedBuf::copy_from(unsafe { self.bytes() })
            .ok_or_else(|| makiri_error("out of memory reading a Ruby string"))
    }
}

/// A live Ruby String's raw bytes, copied into an owned buffer.
pub fn ruby_string_bytes(s: RString) -> Result<OwnedBuf, Error> {
    // SAFETY: `s` is a live Ruby String; the view anchors it for the copy.
    unsafe { ruby_bytes_view(s.as_raw()) }.to_owned_buf()
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
    /// The encoding `str` (a live String) is tagged with.
    fn of(str: VALUE) -> Encoding {
        // SAFETY: `str` is a live String; reading its tag runs no Ruby.
        Encoding(unsafe { rb_sys::rb_enc_get(str) })
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
unsafe fn ruby_to_utf8(str: VALUE) -> VALUE {
    if Encoding::of(str).parses_as_is() {
        return str;
    }
    const REPLACE: c_int = rb_sys::ruby_econv_flag_type::RUBY_ECONV_INVALID_REPLACE as c_int
        | rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_REPLACE as c_int;
    rb_sys::rb_str_encode(
        str,
        rb_sys::rb_enc_from_encoding(Encoding::utf8().0),
        REPLACE,
        rb_sys::Qnil as VALUE,
    )
}

/// [`ruby_to_utf8`] as a safe call: `s` is a live String, and the result is one.
/// An encoding Ruby cannot convert to UTF-8 comes back as its
/// `Encoding::ConverterNotFoundError`, returned rather than raised.
pub fn ruby_to_utf8_value(s: Value) -> Result<Value, Error> {
    // SAFETY: `s` is a live String, and `protect` turns the raise into `Err`.
    let raw = protect(|| unsafe { ruby_to_utf8(s.as_raw()) })?;
    // SAFETY: `rb_str_encode` returns a live String.
    Ok(unsafe { crate::bridge::ruby::value(raw) })
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
    /// `s` must be a String (the caller has coerced it).
    pub fn from_ruby(s: Value) -> Result<HtmlSource, Error> {
        let src = ruby_to_utf8_value(s)?;
        /* A transcode replaced every invalid or unmappable byte, so its result
         * is valid UTF-8 whatever its coderange says. */
        let transcoded = src.as_raw() != s.as_raw();
        // SAFETY: `src` is a live String; the view anchors it.
        let known_valid = transcoded || unsafe { ruby_str_known_valid_utf8(src.as_raw()) };
        // SAFETY: as above.
        let view = unsafe { ruby_bytes_view(src.as_raw()) };
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
unsafe fn ruby_str_known_valid_utf8(str: VALUE) -> bool {
    let Some(r) = RString::from_value(Value::from_raw(str)) else {
        return false;
    };
    match r.enc_coderange() {
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
) -> Result<(RubyText, RubyText), &'static core::ffi::CStr> {
    Ok((
        ruby_try_verified_text(a, max_bytes)?,
        ruby_try_verified_text(b, max_bytes)?,
    ))
}

/// The non-raising form: the checked view, or a static reason on rejection.
/// Allocation-free, like `verify_text`, so the borrow it hands back has not
/// crossed a Ruby allocation. Nothing is coerced: `sv` is a String by type.
pub fn ruby_try_verified_text(
    sv: RString,
    max_bytes: usize,
) -> Result<RubyText, &'static core::ffi::CStr> {
    let sv = sv.as_raw();
    // SAFETY: a live String, by type; the check allocates nothing, and the view
    // anchors the String it borrows from.
    unsafe {
        let (value, ptr, len) = borrow(sv);
        if len > max_bytes {
            return Err(c"string exceeds the maximum length");
        }
        match text_check(sv, ptr, len).reason() {
            Some(reason) => Err(reason),
            None => Ok(RubyText::from_raw_parts(value, ptr, len).locked()),
        }
    }
}
