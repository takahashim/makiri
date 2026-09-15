//! Ruby String <-> Makiri text (bridge/ruby_string.c).
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

/* Every function takes `VALUE`s its caller holds rooted. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_long, CStr};

use magnus::encoding::Coderange;
use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::value::ReprValue;
use magnus::{Error, RString, Value};
use rb_sys::{StableApiDefinition, VALUE};

/// The shared owned buffer.
use crate::cbuf::OwnedBuf;
/// The anchored views, from `glue::abi` - one definition for the whole crate.
/// `RubyText` and `RubyData` are deliberately SEPARATE types: the lattice's whole
/// job is to make a data-family value reaching an engine input a type error.
pub use crate::glue::abi::{RubyBytes, RubyData, RubyText};
/// The UNANCHORED, NUL-permitting slice from `crate::text` - a different type
/// from the Ruby-anchored `glue::abi::RubyText` despite the family resemblance.
/// Text-index slices and Lexbor-interned names reach Ruby through it.
pub use crate::text::BorrowedText;

use crate::bridge::ruby::string_of;
use crate::glue::abi::{error_class, rb_raise, EXC_ERROR};

/* ---- the borrowed-text layouts ----
 *
 * `mkr_ruby_borrowed_text_t` / `_data_t` / `_bytes_t` share ONE layout and are
 * three C types. The distinction is the contract, not the shape: `text` has
 * been checked for valid UTF-8 *and* no NUL, `data` for UTF-8 only (the HTML
 * data family may hold U+0000, like browsers), and `bytes` for nothing at all
 * (HTML parsing decodes leniently). Keeping them apart is what makes a name or
 * engine string that took the data path a type error rather than a silent one,
 * so they stay three types here too: `glue::abi::RubyText` / `RubyData` /
 * `RubyBytes`, one guard type with the contract as its parameter. */

/// What the strict-text check found (`mkr_text_verdict_t`).
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
    let r = RString::from_value(Value::from_raw(s)).expect("a T_STRING");
    let bytes = r.as_slice();
    (s, bytes.as_ptr() as *const c_char, bytes.len())
}

/* ---- assembling Ruby Strings ---- */

/// Join `n` document-order slices totalling `total` bytes into one UTF-8 String.
///
/// This is the text index's output path, so it writes straight into the fresh
/// String's buffer: one pre-sized allocation and one memcpy run, with no
/// intermediate. The bounds checks are not redundant with the caller's
/// bookkeeping - a wrong `total` would otherwise run past the allocation, so
/// both a long slice and a short sum fail closed.
pub unsafe fn ruby_str_from_slices(slices: *const BorrowedText, n: usize, total: usize) -> VALUE {
    if total > c_long::MAX as usize {
        rb_raise(EXC_ERROR.raw(), c"text too large to assemble".as_ptr());
    }
    let str = rb_sys::rb_utf8_str_new(core::ptr::null(), total as c_long);
    /* We just created it and hold the only reference, so writing through the
     * buffer is sound - this is what RSTRING_PTR gives the C. */
    let dst = rb_sys::stable_api::get_default().rstring_ptr(str) as *mut u8;

    let mut off = 0usize;
    for i in 0..n {
        let s = &*slices.add(i);
        if s.is_empty() {
            continue;
        }
        if s.len() > total - off {
            /* off <= total holds, so the subtraction cannot underflow. */
            rb_raise(EXC_ERROR.raw(), c"text slice length inconsistency".as_ptr());
        }
        core::ptr::copy_nonoverlapping(s.as_ptr() as *const u8, dst.add(off), s.len());
        off += s.len();
    }
    if off != total {
        /* A short sum would leave the tail of the uninitialised String unwritten. */
        rb_raise(EXC_ERROR.raw(), c"text slice length inconsistency".as_ptr());
    }
    str
}

/// A UTF-8 String copied from a borrowed slice. NULL is the "absent" sentinel
/// and yields `""` whatever `len` says, so the sentinel is never dereferenced.
pub unsafe fn ruby_str_from_borrowed(text: BorrowedText) -> VALUE {
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
pub fn verify_text(str: Value, what: &CStr) -> Result<(), Error> {
    let str = str.as_raw();
    // SAFETY: `str` is a live String, and the borrow ends with the check -
    // before anything below can allocate.
    let problem = match unsafe {
        let (_, ptr, len) = borrow(str);
        text_check(str, ptr, len)
    } {
        TextVerdict::HasNul => "must not contain a NUL byte",
        TextVerdict::InvalidUtf8 => "must be valid UTF-8",
        TextVerdict::Ok => return Ok(()),
    };
    /* The borrow is not used past the check, so building the message may
     * allocate. */
    Err(text_error(what, problem))
}

/// `Makiri::Error` with "<what> <problem>", the wording the C raised with.
fn text_error(what: &CStr, problem: &str) -> Error {
    let what = what.to_string_lossy();
    Error::new(error_class(), format!("{what} {problem}"))
}

/// Coerce to a String and enforce the strict contract (valid UTF-8, no NUL),
/// naming `what` in the error. The names-and-engine-input path.
pub fn ruby_verified_text(in_: Value, what: &CStr) -> Result<RubyText, Error> {
    let s = string_of(in_)?;
    verify_text(s.as_value(), what)?;
    // SAFETY: `s` is a live String that has just passed the text contract; the
    // view anchors it.
    unsafe {
        let (value, ptr, len) = borrow(s.as_raw());
        Ok(RubyText::from_raw_parts(value, ptr, len))
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
        if text_check(s, ptr, len) == TextVerdict::InvalidUtf8 {
            return Err(text_error(what, "must be valid UTF-8"));
        }
        Ok(RubyData::from_raw_parts(value, ptr, len))
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

/// Copy a String's raw bytes into an owned buffer, so the result is usable
/// while the GVL is released. `None` on OOM, with nothing allocated. `s` must
/// already be a String.
pub unsafe fn ruby_copy_bytes(s: VALUE) -> Option<OwnedBuf> {
    let v = ruby_bytes_view(s);
    /* `v` keeps the String reachable until it drops, after the copy. */
    OwnedBuf::copy_from(v.bytes())
}

/* ---- encoding ---- */

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
pub unsafe fn ruby_to_utf8(str: VALUE) -> VALUE {
    let enc = rb_sys::rb_enc_get(str);
    let utf8 = rb_sys::rb_utf8_encoding();
    if enc == utf8 || enc == rb_sys::rb_usascii_encoding() || enc == rb_sys::rb_ascii8bit_encoding()
    {
        return str;
    }
    const REPLACE: c_int = rb_sys::ruby_econv_flag_type::RUBY_ECONV_INVALID_REPLACE as c_int
        | rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_REPLACE as c_int;
    rb_sys::rb_str_encode(
        str,
        rb_sys::rb_enc_from_encoding(utf8),
        REPLACE,
        rb_sys::Qnil as VALUE,
    )
}

/// Whether Ruby ALREADY knows the String is valid UTF-8.
///
/// This reads the cached classification from the object's flags; it does not
/// scan (a scan would cost as much as running our own validator), so it only
/// wins when Ruby has the answer already. UNKNOWN or BROKEN returns false and
/// the caller validates or sanitises.
pub unsafe fn ruby_str_known_valid_utf8(str: VALUE) -> bool {
    let Some(r) = RString::from_value(Value::from_raw(str)) else {
        return false;
    };
    match r.enc_coderange() {
        /* Every byte < 0x80 in an ASCII-compatible encoding. */
        Coderange::SevenBit => true,
        /* Valid for its own encoding - which has to be UTF-8 for that to mean
         * valid UTF-8. */
        Coderange::Valid => rb_sys::rb_enc_get(str) == rb_sys::rb_utf8_encoding(),
        _ => false,
    }
}

/// The non-raising form: the checked view, or a static reason on rejection.
/// Allocation-free, like `verify_text`, so the borrow it hands back has not
/// crossed a Ruby allocation. `sv` must already be a String; nothing is coerced.
pub unsafe fn ruby_try_verified_text(
    sv: VALUE,
    max_bytes: usize,
) -> Result<RubyText, &'static core::ffi::CStr> {
    let (value, ptr, len) = borrow(sv);
    if len > max_bytes {
        return Err(c"string exceeds the maximum length");
    }
    match text_check(sv, ptr, len) {
        TextVerdict::HasNul => Err(c"string contains a NUL byte"),
        TextVerdict::InvalidUtf8 => Err(c"string is not valid UTF-8"),
        TextVerdict::Ok => Ok(RubyText::from_raw_parts(value, ptr, len)),
    }
}

/* ---- exception messages ---- */

unsafe extern "C" fn exception_message_thunk(exc: VALUE) -> VALUE {
    rb_sys::rb_obj_as_string(rb_sys::rb_funcall(
        exc,
        rb_sys::rb_intern(c"message".as_ptr()),
        0,
    ))
}

/// Write `exc`'s message into `buf` as a NUL-terminated C string, truncating to
/// fit. Falls back to "error" if asking for the message raises or answers with
/// a non-String - this runs on error paths, so it must not raise itself.
pub unsafe fn ruby_exception_message(exc: VALUE, buf: *mut c_char, len: usize) {
    if buf.is_null() || len == 0 {
        return;
    }
    let mut state: c_int = 0;
    let msg = rb_sys::rb_protect(Some(exception_message_thunk), exc, &mut state);
    if state != 0 {
        rb_sys::rb_set_errinfo(rb_sys::Qnil as VALUE);
        return write_cstr(buf, len, b"error");
    }
    let Some(r) = RString::from_value(Value::from_raw(msg)) else {
        return write_cstr(buf, len, b"error");
    };
    /* snprintf("%s") stops at the first NUL, so match that rather than copying
     * the String's full byte length. */
    let bytes = r.as_slice();
    let n = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    write_cstr(buf, len, &bytes[..n]);
}

/// Copy `src` into `buf` (capacity `cap`, including the terminator), truncating
/// as `snprintf` would.
unsafe fn write_cstr(buf: *mut c_char, cap: usize, src: &[u8]) {
    let n = core::cmp::min(src.len(), cap - 1);
    core::ptr::copy_nonoverlapping(src.as_ptr(), buf as *mut u8, n);
    *buf.add(n) = 0;
}
