//! Makiri's bytes made into Ruby Strings.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_long};

use crate::bridge::ruby::makiri_error;
/* Not magnus's: ours catches a panic before `rb_protect`'s C frame. */
use magnus::{Error, RString, Value};
use rb_sys::{StableApiDefinition, VALUE};

/* ---- assembling Ruby Strings ---- */

/// A UTF-8 String holding a copy of `bytes`. Ruby's allocator raises
/// `NoMemoryError`, a longjmp past every Rust frame up to the nearest
/// `rb_protect`, so this is for a frame that owns nothing a skipped `Drop`
/// would leak; a frame that does uses [`utf8_str_protected`].
/// `rake unsafe:boundaries` holds the glue to that.
///
/// `bytes` must be valid UTF-8 (the String is tagged so without a check):
/// everything a document lends is, by the text-input contract, and so is
/// every checked view. Bytes that were not would make a wrong String, never
/// a memory error.
pub fn utf8_str(bytes: &[u8]) -> Value {
    // SAFETY: valid UTF-8 by this function's contract; the String copies it.
    unsafe { crate::bridge::ruby::value(ruby_str_from_utf8(bytes)) }
}

/// [`utf8_str`] for a frame that owns what a raise would leak (a serializer's
/// output buffer, an argument's copy): the allocator's raise comes back as
/// `Err`, and the caller's owned values drop.
pub fn utf8_str_protected(bytes: &[u8]) -> Result<RString, Error> {
    if c_long::try_from(bytes.len()).is_err() {
        return Err(makiri_error("string too large"));
    }
    // SAFETY: valid UTF-8 by `utf8_str`'s contract; `protect_value` turns the
    // allocator's raise into `Err`.
    let v = crate::bridge::ruby::protect_value(|| unsafe { ruby_str_from_utf8(bytes) })?;
    RString::from_value(v)
        .ok_or_else(|| crate::bridge::ruby::internal_error("the allocator made no String"))
}

/// Join document-order slices totalling `total` bytes into one UTF-8 String.
///
/// This is the text index's output path, so it writes straight into the fresh
/// String's buffer: one pre-sized allocation and one memcpy run, with no
/// intermediate. The bounds checks are not redundant with the caller's
/// bookkeeping - a wrong `total` would otherwise run past the allocation, so
/// both a long slice and a short sum fail closed. The joined bytes are to be
/// valid UTF-8, as text-index slices are by the text-input contract; bytes
/// that were not would make a wrong String, not a memory error.
///
/// # Safety
/// Allocating the String RAISES on failure (`raw::utf8_str_new`), a longjmp
/// over every frame up to the nearest `rb_protect`: call it under
/// `bridge::ruby::protect`, or from a frame that owns nothing a skipped
/// `Drop` would leak. The returned VALUE is unrooted until stored or returned.
pub unsafe fn ruby_str_from_slices<'a>(
    slices: impl Iterator<Item = &'a [u8]>,
    total: usize,
) -> Result<VALUE, Error> {
    if total > c_long::MAX as usize {
        return Err(makiri_error("text too large to assemble"));
    }
    let str = crate::bridge::raw::utf8_str_new(core::ptr::null(), total as c_long);
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
/// because only the caller knows where they came from: everything in a
/// parsed document is valid UTF-8 by the text-input contract, and bytes that
/// were not would make a wrong String - wrong is still wrong - not a memory
/// error.
///
/// # Safety
/// Allocating the String RAISES on failure (`raw::utf8_str_new`), as an
/// over-long `bytes` does: call it under `bridge::ruby::protect`, or from a
/// frame that owns nothing a skipped `Drop` would leak.
pub unsafe fn ruby_str_from_utf8(bytes: &[u8]) -> VALUE {
    /* A length `c_long` cannot hold (32 bits on Windows) is passed as -1,
     * which Ruby refuses with ArgumentError - a raise, as an allocation
     * failure already is - rather than truncated into a shorter String. */
    let len = c_long::try_from(bytes.len()).unwrap_or(-1);
    /* A slice's pointer is never null, even when it is empty. */
    crate::bridge::raw::utf8_str_new(bytes.as_ptr() as *const c_char, len)
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
    /* A length `c_long` cannot hold (32 bits on Windows) would be truncated,
     * or turn negative, which `rb_enc_interned_str` does not check: refused
     * as `utf8_str` refuses it, with ArgumentError. */
    let Ok(len) = c_long::try_from(bytes.len()) else {
        return utf8_str(bytes);
    };
    // SAFETY: a slice's pointer and length, read for the call; the encoding
    // is Ruby's static UTF-8 one. The result is a live String.
    unsafe {
        crate::bridge::ruby::value(crate::bridge::raw::enc_interned_str(
            bytes.as_ptr() as *const c_char,
            len,
            rb_sys::rb_utf8_encoding(),
        ))
    }
}
