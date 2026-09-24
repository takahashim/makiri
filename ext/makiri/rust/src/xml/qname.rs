//! QName splitting and xmlns detection over byte slices: the XML naming rules
//! and nothing else.
//!
//! What used to share the file has moved to where its one consumer is - the XML
//! declaration's pseudo-attribute grammars to `tree`, the leaf-value forbidden
//! sequences to `mutate`, and the WHATWG DOM's much looser element names, which
//! are not XML naming at all, to [`crate::xml::dom_name`].

#![forbid(unsafe_code)]

use crate::xml::chars::{decode1, is_name_start, validate_name};
use crate::xml::{XMLNS_NS_URI, XML_NS_URI};

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

/// Whether `ns` may name the namespace of an attribute called `name` (split per
/// `sp`) - the DOM's "validate and extract": a prefix needs a namespace, `xml`
/// and its namespace take only each other, and `xmlns` (as the name or the
/// prefix) takes only the XMLNS namespace, which in turn takes nothing else. `set_attribute_ns`
/// checked none of it, and wrote `xmlns:p=""` or an `xml:` attribute that
/// re-read in another namespace.
pub fn ns_fits_name(ns: &[u8], name: &[u8], sp: &Split) -> bool {
    let prefix = &name[..sp.prefix_len as usize];
    let is_xmlns = name == b"xmlns" || prefix == b"xmlns";
    if !prefix.is_empty() && ns.is_empty() {
        return false;
    }
    /* The DOM stops at "xml takes only its own namespace"; the converse is
     * Namespaces in XML's (§3: the XML namespace is bound to no other prefix),
     * and without it the attribute could only be written under a declaration
     * no parser accepts. */
    if (prefix == b"xml") != (ns == XML_NS_URI) {
        return false;
    }
    is_xmlns == (ns == XMLNS_NS_URI)
}

/// Whether `prefix` - empty for the default `xmlns` - may be declared for
/// `uri` (Namespaces in XML 1.0 §3): `xmlns` is never declared, `xml` only for
/// its own URI, neither reserved URI for anything else, and no prefix for the
/// empty URI (only the default may be undeclared that way).
///
/// The one statement of the rule. The parser always applied it; the mutators
/// applied only the last clause, so `[]=`, `set_attribute_ns` and `rename`
/// could write `xmlns:xml="urn:other"` into a tree `to_xml` then could not
/// re-read. [`ns_decl_check`] says which clause a refusal broke.
pub fn ns_decl_ok(prefix: &[u8], uri: &[u8]) -> bool {
    ns_decl_check(prefix, uri).is_ok()
}

/// Which clause of the §3 declaration rule a declaration breaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NsDeclError {
    /// `xmlns:xmlns`: the `xmlns` prefix is never declared.
    Xmlns,
    /// `xmlns:xml` with a URI other than the XML namespace.
    XmlElsewhere,
    /// The XML or XMLNS namespace bound to another prefix (`default: false`)
    /// or made the default namespace (`default: true`).
    ReservedUri { default: bool },
    /// A prefix bound to the empty namespace; only the default may be undeclared.
    PrefixToEmpty,
}

/// [`ns_decl_ok`], saying which clause failed.
pub fn ns_decl_check(prefix: &[u8], uri: &[u8]) -> Result<(), NsDeclError> {
    if prefix == b"xmlns" {
        return Err(NsDeclError::Xmlns);
    }
    if prefix == b"xml" {
        return if uri == XML_NS_URI {
            Ok(())
        } else {
            Err(NsDeclError::XmlElsewhere)
        };
    }
    if uri == XML_NS_URI || uri == XMLNS_NS_URI {
        return Err(NsDeclError::ReservedUri {
            default: prefix.is_empty(),
        });
    }
    if prefix.is_empty() || !uri.is_empty() {
        Ok(())
    } else {
        Err(NsDeclError::PrefixToEmpty)
    }
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
