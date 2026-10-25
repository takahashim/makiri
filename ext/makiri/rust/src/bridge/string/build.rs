//! Makiri's bytes made into Ruby Strings.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, ReprValue, Value};
use crate::bridge::ruby::makiri_error;

/* ---- assembling Ruby Strings ---- */

pub use crate::nupur::build::{utf8_rstring, utf8_str};

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
    ruby: &Ruby,
    slices: impl Iterator<Item = &'a [u8]>,
    total: usize,
) -> Result<Value, Error> {
    let str = crate::bridge::raw::utf8_str_filled(ruby, total, |out| {
        for s in slices {
            /* A slice past `total` is refused: `push` writes nothing past
             * the String's end. */
            if !out.push(s) {
                return Err(makiri_error("text slice length inconsistency"));
            }
        }
        /* A short sum is `utf8_str_filled`'s to refuse: it holds the String
         * to its length. */
        Ok(())
    })?;
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
pub fn dom_name_str(ruby: &Ruby, bytes: &[u8]) -> Result<Value, Error> {
    crate::bridge::raw::enc_interned_str(ruby, bytes, super::Encoding::utf8(ruby))
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
    ruby: &Ruby,
    read: G,
    kind: StrKind,
    bytes: impl FnOnce(&G) -> Option<&[u8]>,
) -> Result<Option<Value>, Error> {
    let Some(b) = bytes(&read) else {
        return Ok(None);
    };
    match kind {
        StrKind::Utf8 => utf8_str(ruby, b),
        StrKind::DomName => dom_name_str(ruby, b),
    }
    .map(Some)
}
