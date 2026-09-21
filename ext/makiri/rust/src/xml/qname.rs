//! QName splitting and xmlns detection over byte slices: the XML naming rules
//! and nothing else.
//!
//! What used to share the file has moved to where its one consumer is - the XML
//! declaration's pseudo-attribute grammars to `tree`, the leaf-value forbidden
//! sequences to `mutate`, and the WHATWG DOM's much looser element names, which
//! are not XML naming at all, to [`crate::xml::dom_name`].

#![forbid(unsafe_code)]

use crate::xml::chars::{decode1, is_name_start, validate_name};

/// A QName split into its parts as OFFSETS into the name (prefix is always
/// at offset 0; prefix_len 0 = unprefixed).
///
/// The lengths are `u32` because the arena stores them as `Span`s, and a name
/// that long is refused before it gets here: the parser's cursor caps every
/// scanned slice at `u32::MAX` and the bridge caps a programmatic name at the
/// Ruby String's length. Build one through [`Split::unprefixed`] or
/// [`Split::prefixed`] rather than by hand, so `local_off` cannot disagree with
/// `prefix_len`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub prefix_len: u32,
    pub local_off: u32,
    pub local_len: u32,
}

impl Split {
    /// A name with no prefix: all `len` bytes are the local part.
    pub const fn unprefixed(len: u32) -> Split {
        Split {
            prefix_len: 0,
            local_off: 0,
            local_len: len,
        }
    }

    /// A prefixed name, `prefix ':' local`: the one place that knows the local
    /// part starts one byte past the prefix.
    pub const fn prefixed(prefix_len: u32, local_len: u32) -> Split {
        Split {
            prefix_len,
            local_off: prefix_len + 1,
            local_len,
        }
    }

    /// The length of the qualified name this split describes.
    pub const fn qname_len(&self) -> usize {
        self.local_off as usize + self.local_len as usize
    }

    /// Whether `qname` really is this split's prefix, a colon, and its local
    /// part - the inverse of building the split, for a caller handed all three
    /// separately.
    pub fn describes(&self, qname: &[u8], prefix: &[u8], local: &[u8]) -> bool {
        let (pl, lo) = (self.prefix_len as usize, self.local_off as usize);
        qname.len() == self.qname_len()
            && qname.get(..pl) == Some(prefix)
            && (pl == lo || qname.get(pl) == Some(&b':'))
            && qname.get(lo..) == Some(local)
    }
}

/// Split a name whose bytes are ALREADY known to be a valid XML 1.0 Name,
/// enforcing the NCName rules a QName adds: at most one colon, non-empty
/// prefix and local, and a local part beginning with a NameStartChar.
/// `name.len()` must fit in u32 (callers pass
/// u32 lengths).
pub fn split_scanned(name: &[u8]) -> Option<Split> {
    let len = name.len();
    match name.iter().position(|&b| b == b':') {
        None => Some(Split {
            prefix_len: 0,
            local_off: 0,
            local_len: len as u32,
        }),
        Some(pl) => {
            let ll = len - pl - 1;
            if pl == 0 || ll == 0 {
                return None; /* ":x" or "x:" */
            }
            let local = &name[pl + 1..];
            if local.contains(&b':') {
                return None; /* a second colon */
            }
            let (cp, _) = decode1(local)?;
            if !is_name_start(cp) {
                return None; /* local must be an NCName */
            }
            Some(Split::prefixed(pl as u32, ll as u32))
        }
    }
}

/// Validate `name` as a full XML 1.0 Name, then split it (the mutation path,
/// whose input is not pre-scanned).
pub fn split_checked(name: &[u8]) -> Option<Split> {
    if name.is_empty() || !validate_name(name) {
        return None;
    }
    split_scanned(name)
}

/// If `name` is an xmlns declaration ("xmlns" / "xmlns:PREFIX"), the declared
/// prefix.
///
/// The prefix is EMPTY for `xmlns` itself, which declares the default
/// namespace. That is the representation every caller wants - a binding is
/// keyed by prefix and "" is the default's key throughout the parser, the
/// serializer and `resolve_in_scope` - so it stays a slice rather than becoming
/// a `Default | Prefix(..)` enum every one of them would immediately flatten.
#[inline]
pub fn xmlns_prefix(name: &[u8]) -> Option<&[u8]> {
    if name == b"xmlns" {
        Some(&name[5..])
    } else if name.len() > 6 && name.starts_with(b"xmlns:") {
        Some(&name[6..])
    } else {
        None
    }
}
