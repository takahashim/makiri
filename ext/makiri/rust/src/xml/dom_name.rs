//! WHATWG DOM element names: what `createElement` accepts, which is far looser
//! than an XML Name.
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
