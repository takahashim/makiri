//! Makiri's bytes made into Ruby Strings.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, ReprValue, Value};

/* ---- assembling Ruby Strings ---- */

pub use crate::nupur::string::utf8_str;

/// Join the text index's document-order slices, totalling `total` bytes,
/// into one UTF-8 String (`nupur::string::utf8_str_from_slices`). The joined
/// bytes are UTF-8, as text-index slices are by the text-input contract.
pub fn utf8_str_from_slices<'a>(
    ruby: &Ruby,
    slices: impl Iterator<Item = &'a [u8]>,
    total: usize,
) -> Result<Value, Error> {
    crate::nupur::string::utf8_str_from_slices(ruby, slices, total).map(|s| s.as_value())
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
/// Every caller passes a document's name, which is UTF-8 by the text-input
/// contract (`nupur::string::interned_utf8`).
pub fn dom_name_str(ruby: &Ruby, bytes: &[u8]) -> Result<Value, Error> {
    crate::nupur::string::interned_utf8(ruby, bytes).map(|s| s.as_value())
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
        StrKind::Utf8 => utf8_str(ruby, b).map(|s| s.as_value()),
        StrKind::DomName => dom_name_str(ruby, b),
    }
    .map(Some)
}
