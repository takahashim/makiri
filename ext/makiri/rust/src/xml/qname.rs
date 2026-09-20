//! QName splitting and xmlns detection over byte slices. No unsafe code.

#![forbid(unsafe_code)]

use crate::xml::chars::{decode1, is_name_start, validate_name};
use crate::xml::NodeType;

/// A QName split into its parts as OFFSETS into the name (prefix is always
/// at offset 0; prefix_len 0 = unprefixed).
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
            Some(Split {
                prefix_len: pl as u32,
                local_off: (pl + 1) as u32,
                local_len: ll as u32,
            })
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
/// prefix (empty for the default namespace).
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

/// XML declaration pseudo-attribute value grammars (§2.8).
pub fn is_version_num(s: &[u8]) -> bool {
    s.len() >= 3 && s.starts_with(b"1.") && s[2..].iter().all(|b| b.is_ascii_digit())
}

pub fn is_enc_name(s: &[u8]) -> bool {
    match s.first() {
        Some(c0) if c0.is_ascii_alphabetic() => s[1..]
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-'),
        _ => false,
    }
}

pub fn is_yes_no(s: &[u8]) -> bool {
    s == b"yes" || s == b"no"
}

/// Forbidden character SEQUENCE for a leaf value: "--" (or a trailing "-") in
/// a comment, "]]>" in CDATA, "?>" in a PI.
pub fn value_seq_ok(node_type: NodeType, text: &[u8]) -> bool {
    match node_type {
        NodeType::Comment => text.last() != Some(&b'-') && !text.windows(2).any(|w| w == b"--"),
        NodeType::CData => !text.windows(3).any(|w| w == b"]]>"),
        NodeType::Pi => !text.windows(2).any(|w| w == b"?>"),
        _ => true,
    }
}

/* ---- DOM-loose element names (WHATWG DOM, not XML) ---- */

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
fn dom_name_forbidden(c: u8) -> bool {
    matches!(c, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ' | b'/' | b'>')
}

fn dom_prefix_ok(p: &[u8]) -> bool {
    !p.is_empty() && !p.iter().copied().any(dom_name_forbidden)
}

fn dom_local_ok(p: &[u8]) -> bool {
    let Some(&first) = p.first() else {
        return false;
    };
    if first < 0x80 && !(first.is_ascii_alphabetic() || first == b':' || first == b'_') {
        return false;
    }
    !p.iter().copied().any(dom_name_forbidden)
}

/// Check that `qname`, `prefix` and `local` describe one DOM element name -
/// valid under the WHATWG rules, and `qname` exactly `prefix:local` (or
/// `local` when there is no prefix) - and split it.
///
/// For the browser-DOM escape hatch that makes an element XML cannot name
/// (`Document#create_loose_dom_element`); the serializer refuses those later.
pub fn split_loose_dom_name(
    qname: &[u8],
    prefix: Option<&[u8]>,
    local: &[u8],
) -> Result<Split, LooseNameError> {
    if !dom_local_ok(local) {
        return Err(LooseNameError::Local);
    }
    let Some(p) = prefix else {
        if qname != local {
            return Err(LooseNameError::UnprefixedMismatch);
        }
        return Ok(Split::unprefixed(qname.len() as u32));
    };
    if !dom_prefix_ok(p) {
        return Err(LooseNameError::Prefix);
    }
    if qname.len() != p.len() + 1 + local.len()
        || &qname[..p.len()] != p
        || qname[p.len()] != b':'
        || &qname[p.len() + 1..] != local
    {
        return Err(LooseNameError::PrefixedMismatch);
    }
    Ok(Split {
        prefix_len: p.len() as u32,
        local_off: (p.len() + 1) as u32,
        local_len: local.len() as u32,
    })
}
