//! QName splitting and xmlns detection over byte slices. No unsafe code.

#![forbid(unsafe_code)]

use crate::chars::{decode1, is_name_start, validate_name};

/// A QName split into its parts as OFFSETS into the name (prefix is always
/// at offset 0; prefix_len 0 = unprefixed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    pub prefix_len: u32,
    pub local_off: u32,
    pub local_len: u32,
}

/// Split a name whose bytes are ALREADY known to be a valid XML 1.0 Name,
/// enforcing the NCName rules a QName adds: at most one colon, non-empty
/// prefix and local, and a local part beginning with a NameStartChar.
/// mkr_xml_split_scanned_qname. `name.len()` must fit in u32 (callers pass
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
/// whose input is not pre-scanned). mkr_xml_qname_split.
pub fn split_checked(name: &[u8]) -> Option<Split> {
    if name.is_empty() || !validate_name(name) {
        return None;
    }
    split_scanned(name)
}

/// If `name` is an xmlns declaration ("xmlns" / "xmlns:PREFIX"), the declared
/// prefix (empty for the default namespace). mkr_xml_xmlns_prefix.
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
/// a comment, "]]>" in CDATA, "?>" in a PI. mkr_xml_check_value_seq.
pub fn value_seq_ok(node_type: u32, text: &[u8]) -> bool {
    match node_type {
        crate::T_COMMENT => text.last() != Some(&b'-') && !text.windows(2).any(|w| w == b"--"),
        crate::T_CDATA => !text.windows(3).any(|w| w == b"]]>"),
        crate::T_PI => !text.windows(2).any(|w| w == b"?>"),
        _ => true,
    }
}
