//! Makiri's bytes made into Ruby Strings.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::bridge::rb::value::AsRawValue;
use crate::bridge::rb::{Error, RString, ReprValue, Value};
use crate::bridge::ruby::{gvl_ruby, makiri_error};
use rb_sys::StableApiDefinition;

/* ---- assembling Ruby Strings ---- */

/// A UTF-8 String holding a copy of `bytes`; a failed allocation is `Err`
/// (`NoMemoryError`), as `raw`'s every call is.
///
/// `bytes` must be valid UTF-8 (the String is tagged so without a check):
/// everything a document lends is, by the text-input contract, and so is
/// every checked view. Bytes that were not would make a wrong String, never
/// a memory error.
pub fn utf8_str(bytes: &[u8]) -> Result<Value, Error> {
    utf8_rstring(bytes).map(|s| s.as_value())
}

/// [`utf8_str`] as an `RString`.
pub fn utf8_rstring(bytes: &[u8]) -> Result<RString, Error> {
    crate::bridge::raw::utf8_str_new(&gvl_ruby(), bytes)
}

/// Join document-order slices totalling `total` bytes into one UTF-8 String.
///
/// This is the text index's output path, so it writes straight into the fresh
/// String's buffer: one pre-sized allocation and one copy run, with no
/// intermediate. The bounds checks are not redundant with the caller's
/// bookkeeping - a wrong `total` would otherwise run past the allocation, so
/// both a long slice and a short sum fail closed. The joined bytes are to be
/// valid UTF-8, as text-index slices are by the text-input contract; bytes
/// that were not would make a wrong String, not a memory error.
pub fn utf8_str_from_slices<'a>(
    slices: impl Iterator<Item = &'a [u8]>,
    total: usize,
) -> Result<Value, Error> {
    // SAFETY: every one of the `total` bytes is written below before the
    // String is returned - a short or long sum is refused - and nothing
    // reads it before.
    let str = unsafe { crate::bridge::raw::utf8_str_unwritten(&gvl_ruby(), total) }?;
    /* We just created it and hold the only reference, so writing through the
     * buffer is sound - this is what RSTRING_PTR gives the C. It has room for
     * exactly `total` bytes, and nothing else runs while `out` is held. */
    // SAFETY: as above; `str` is a live String of `total` bytes.
    let out = unsafe {
        let dst = rb_sys::stable_api::get_default().rstring_ptr(str.as_raw()) as *mut u8;
        core::slice::from_raw_parts_mut(dst, total)
    };
    let mut off = 0usize;
    for s in slices {
        /* A slice past `total` is refused by the bounds check. */
        let Some(to) = off
            .checked_add(s.len())
            .and_then(|end| out.get_mut(off..end))
        else {
            return Err(makiri_error("text slice length inconsistency"));
        };
        /* Text slices are mostly a few bytes: fixed-size copies, not a
         * `memcpy` call each (`cbuf::copy_small`). */
        crate::cbuf::copy_small(to, s);
        off += s.len();
    }
    if off != total {
        /* A short sum would leave the tail of the uninitialised String unwritten. */
        return Err(makiri_error("text slice length inconsistency"));
    }
    Ok(str.as_value())
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
pub fn dom_name_str(bytes: &[u8]) -> Result<Value, Error> {
    crate::bridge::raw::enc_interned_str(&gvl_ruby(), bytes, super::Encoding::utf8())
        .map(|s| s.as_value())
}

/* ---- a String of bytes a document read lends ---- */

/// How [`str_from_read`] makes its String: a fresh UTF-8 one ([`utf8_str`]),
/// or an interned DOM name ([`dom_name_str`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrKind {
    Utf8,
    DomName,
}

/// The String `bytes` - borrowed from `read`, a document read guard - make,
/// made while the guard is held: a failed allocation is `Err`, so the guard
/// drops either way. `bytes` sees the guard and answers what to make the
/// String of, or `None` for nil.
#[inline]
pub fn str_from_read<G>(
    read: G,
    kind: StrKind,
    bytes: impl FnOnce(&G) -> Option<&[u8]>,
) -> Result<Option<Value>, Error> {
    let Some(b) = bytes(&read) else {
        return Ok(None);
    };
    match kind {
        StrKind::Utf8 => utf8_str(b),
        StrKind::DomName => dom_name_str(b),
    }
    .map(Some)
}
