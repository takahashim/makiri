//! WHATWG DOM names: what `createElement` and `setAttribute` accept, which is
//! far looser than an XML Name - but not unchecked. The HTML mutators apply it
//! to every element and attribute name they are given.
//!
//! Its own module because it is not XML naming. `Document#create_loose_dom_element`
//! exists so a caller can build the element a browser would - `":good:times:"`,
//! `"x<"` - and the XML serializer refuses such a name later
//! (`FLAG_DOM_LOOSE_NAME`, `serialize::Failure::DomLooseName`). Keeping it out
//! of [`crate::xml::qname`] means an XML rule can never be relaxed by reading
//! this file's rules as the same thing.

#![forbid(unsafe_code)]

use crate::xml::qname::Split;

/// Why [`split_loose_dom_name`] refused a name. The bridge words it as an
/// `ArgumentError`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LooseNameError {
    Local,
    Prefix,
    UnprefixedMismatch,
    PrefixedMismatch,
}

impl LooseNameError {
    pub fn message(self) -> &'static str {
        match self {
            LooseNameError::Local => "invalid DOM element local name",
            LooseNameError::Prefix => "invalid DOM element prefix",
            LooseNameError::UnprefixedMismatch => {
                "qualified name must equal local name when prefix is nil"
            }
            LooseNameError::PrefixedMismatch => "qualified name must be prefix + ':' + local name",
        }
    }
}

/// The WHATWG DOM's name-character exclusions: what `createElement` refuses
/// even though it is far looser than an XML Name.
fn forbidden(c: u8) -> bool {
    matches!(c, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ' | b'/' | b'>')
}

fn prefix_ok(p: &[u8]) -> bool {
    !p.is_empty() && !p.iter().copied().any(forbidden)
}

fn local_ok(p: &[u8]) -> bool {
    let Some(&first) = p.first() else {
        return false;
    };
    if first < 0x80 && !(first.is_ascii_alphabetic() || first == b':' || first == b'_') {
        return false;
    }
    !p.iter().copied().any(forbidden)
}

/// Whether `name` is a WHATWG DOM "valid element local name" - what
/// `createElement` accepts.
pub fn valid_element_local_name(name: &[u8]) -> bool {
    local_ok(name)
}

/// Whether `name` is a WHATWG DOM "valid attribute local name": at least one
/// character, and no ASCII whitespace, NUL, `/`, `=` or `>` - the characters
/// that would end or split the name when the element is serialized.
pub fn valid_attribute_local_name(name: &[u8]) -> bool {
    !name.is_empty() && !name.iter().any(|&c| forbidden(c) || c == b'=')
}

/// Whether `prefix` is a WHATWG DOM "valid namespace prefix".
pub fn valid_namespace_prefix(prefix: &[u8]) -> bool {
    prefix_ok(prefix)
}

/// Check that `qname`, `prefix` and `local` describe one DOM element name -
/// valid under the WHATWG rules, and `qname` exactly `prefix:local` (or
/// `local` when there is no prefix) - and split it.
pub fn split_loose_dom_name(
    qname: &[u8],
    prefix: Option<&[u8]>,
    local: &[u8],
) -> Result<Split, LooseNameError> {
    if !local_ok(local) {
        return Err(LooseNameError::Local);
    }
    let Some(p) = prefix else {
        let split = Split::unprefixed(qname.len() as u32);
        if !split.describes(qname, b"", local) {
            return Err(LooseNameError::UnprefixedMismatch);
        }
        return Ok(split);
    };
    if !prefix_ok(p) {
        return Err(LooseNameError::Prefix);
    }
    let split = Split::prefixed(p.len() as u32, local.len() as u32);
    if !split.describes(qname, p, local) {
        return Err(LooseNameError::PrefixedMismatch);
    }
    Ok(split)
}
