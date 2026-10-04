//! QName splitting and xmlns detection over byte slices: the XML naming rules
//! and nothing else.
//!
//! What used to share the file has moved to where its one consumer is - the XML
//! declaration's pseudo-attribute grammars to `tree`, the leaf-value forbidden
//! sequences to `mutate`, and the WHATWG DOM's much looser element names, which
//! are not XML naming at all, to [`crate::xml::dom_name`].

#![forbid(unsafe_code)]

use crate::xml::chars::{decode1, is_name_start, validate_name};
use crate::xml::{Document, Span, XMLNS_NS_URI, XML_NS_URI};

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

/// Whether `prefix` - empty for the default `xmlns` - may be declared for
/// `uri` (Namespaces in XML 1.0 §3): `xmlns` is never declared, `xml` only for
/// its own URI, neither reserved URI for anything else, and no prefix for the
/// empty URI (only the default may be undeclared that way) - `Err` naming the
/// clause broken.
///
/// The one statement of the rule, for the parser and the mutators alike. The
/// mutators once applied only the last clause, so `[]=`, `set_attribute_ns`
/// and the since-removed `rename` could write `xmlns:xml="urn:other"` into a
/// tree `to_xml` then could not re-read.
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

/* ---- a name's namespace (Namespaces in XML §5, §6) ---- */

/// Whether a name is an element's or an attribute's, which Namespaces in XML
/// treats differently: an unprefixed attribute is in no namespace and an
/// unprefixed element in the default one, and only an attribute can be a
/// declaration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NameRole {
    Element,
    Attribute,
}

/// The namespace a name is in, by [`name_ns`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NameNs {
    /// The URI, `Span::EMPTY` for no namespace.
    Uri(Span),
    /// A prefix nothing in scope binds: the caller's to refuse (the parser,
    /// a connected placement) or to leave pending (a detached build).
    Unbound,
}

/// An element named with the `xmlns` prefix, which Namespaces in XML reserves
/// for declarations (§3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ReservedPrefix;

/// The namespace `name` (split per `sp`) is in, as `role`, where `lookup`
/// answers the URI bound to a prefix in scope ("" for the default namespace).
///
/// Namespaces in XML's rules in one place, for the parser and the mutators
/// alike - only WHERE a prefix is looked up (the parser's open scope, a
/// placement's ancestors) and what an unbound one means are theirs:
/// an `xmlns` / `xmlns:p` attribute is in the XMLNS namespace; the `xml`
/// prefix is bound to the XML namespace with no declaration; an unprefixed
/// attribute is in none and an unprefixed element in the default namespace,
/// if one is in scope (an empty binding undeclares it); an element may not
/// carry the `xmlns` prefix.
pub(crate) fn name_ns(
    doc: &Document,
    name: &[u8],
    sp: &Split,
    role: NameRole,
    lookup: impl Fn(&[u8]) -> Option<Span>,
) -> Result<NameNs, ReservedPrefix> {
    if role == NameRole::Attribute && xmlns_prefix(name).is_some() {
        return Ok(NameNs::Uri(doc.xmlns_ns_span()));
    }
    let bound = |prefix: &[u8]| lookup(prefix).filter(|s| s.len > 0);
    let prefix = &name[..sp.prefix_len as usize];
    if prefix.is_empty() {
        return Ok(NameNs::Uri(match role {
            NameRole::Attribute => Span::EMPTY,
            NameRole::Element => bound(b"").unwrap_or(Span::EMPTY),
        }));
    }
    if prefix == b"xml" {
        return Ok(NameNs::Uri(doc.xml_ns_span()));
    }
    if prefix == b"xmlns" {
        return Err(ReservedPrefix);
    }
    Ok(bound(prefix).map_or(NameNs::Unbound, NameNs::Uri))
}
