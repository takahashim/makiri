//! WHATWG DOM names: what `createElement` and `setAttribute` accept, which is
//! far looser than an XML Name - but not unchecked. The HTML mutators apply it
//! to every element and attribute name they are given.
//!
//! Its own module because it is not XML naming. `Document#create_element_ns`
//! and `create_loose_dom_element` exist so a caller can build the element a
//! browser would - `"f}oo"`, `"x<"` - and the XML serializer refuses such a
//! name later
//! (`NodeFlags::DOM_LOOSE_NAME`, `serialize::Failure::DomLooseName`). Keeping it out
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

/// A name a WHATWG DOM name rule accepted, and so never empty: what the
/// factories that make an element or an attribute take, so an empty name
/// cannot reach one. Made only here - by a rule ([`Name::element`],
/// [`Name::attribute`], [`validate_and_extract`], [`split_loose_dom_name`]),
/// or, for a name another tree already holds, by [`Name::held`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name<'a>(&'a [u8]);

impl<'a> Name<'a> {
    /// `name`, if it is a valid element local name.
    pub fn element(name: &'a [u8]) -> Option<Name<'a>> {
        valid_element_local_name(name).then_some(Name(name))
    }

    /// `name`, if it is a valid attribute local name.
    pub fn attribute(name: &'a [u8]) -> Option<Name<'a>> {
        valid_attribute_local_name(name).then_some(Name(name))
    }

    /// A name another tree holds - an imported node's - which that tree's own
    /// rules made, held here only to not being empty.
    pub fn held(name: &'a [u8]) -> Option<Name<'a>> {
        (!name.is_empty()).then_some(Name(name))
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }
}

/// A qualified name split at its first colon: as written, its prefix (empty
/// for none) and its local name, which is a [`Name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QName<'q> {
    pub written: &'q [u8],
    pub prefix: &'q [u8],
    pub local: Name<'q>,
}

impl<'q> QName<'q> {
    /// `name` unsplit: no prefix, the whole name the local name - a name
    /// set with no namespace, which a colon does not split.
    pub fn whole(name: Name<'q>) -> QName<'q> {
        QName {
            written: name.as_bytes(),
            prefix: b"",
            local: name,
        }
    }

    /// A qualified name another tree holds ([`Name::held`]), split at its
    /// first colon when `split` - when it is in a namespace.
    pub fn held(written: &'q [u8], split: bool) -> Option<QName<'q>> {
        let colon = if split {
            written.iter().position(|&b| b == b':')
        } else {
            None
        };
        Some(match colon {
            Some(i) => QName {
                written,
                prefix: &written[..i],
                local: Name::held(&written[i + 1..])?,
            },
            None => QName::whole(Name::held(written)?),
        })
    }
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

/// Whether `name` is a WHATWG DOM "valid doctype name" - what
/// `createDocumentType` checks, and all it checks: no ASCII whitespace, NUL or
/// `>`. The DOM allows the empty name; Makiri refuses it, as the HTML factory
/// must (Lexbor reads an empty name as absent), so both answer alike.
pub fn valid_doctype_name(name: &[u8]) -> bool {
    !name.is_empty()
        && !name
            .iter()
            .any(|&c| matches!(c, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ' | b'>'))
}

/// Whether `prefix` is a WHATWG DOM "valid namespace prefix".
pub fn valid_namespace_prefix(prefix: &[u8]) -> bool {
    prefix_ok(prefix)
}

/// Why [`validate_and_extract`] refused a name: a half the naming rule
/// refuses (the DOM's InvalidCharacterError), or a namespace that does not
/// fit the name (NamespaceError).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractError {
    Name,
    Namespace,
}

/// The DOM's "validate and extract": `qname` split at its first colon into
/// (prefix, local name) - prefix empty when there is none - with the prefix
/// held to "valid namespace prefix", the local name to `local_ok` (the
/// element or the attribute rule), and then `ns` ("" = null) to the name
/// ([`namespace_fits`]). One body for every caller, so the rule is changed in
/// one place.
pub fn validate_and_extract<'q>(
    ns: &[u8],
    qname: &'q [u8],
    local_ok: fn(&[u8]) -> bool,
) -> Result<QName<'q>, ExtractError> {
    let (prefix, local) = match qname.iter().position(|&b| b == b':') {
        Some(i) => {
            let (prefix, local) = (&qname[..i], &qname[i + 1..]);
            if !valid_namespace_prefix(prefix) {
                return Err(ExtractError::Name);
            }
            (prefix, local)
        }
        None => (&b""[..], qname),
    };
    if !local_ok(local) {
        return Err(ExtractError::Name);
    }
    if !namespace_fits(ns, qname, prefix) {
        return Err(ExtractError::Namespace);
    }
    /* Every DOM local-name rule refuses the empty name. */
    let local = Name::held(local).ok_or(ExtractError::Name)?;
    Ok(QName {
        written: qname,
        prefix,
        local,
    })
}

/// Whether `ns` ("" = null) fits a name with this `prefix` ("" = none) - the
/// namespace clauses of the DOM's "validate and extract": a prefix needs a
/// namespace, `xml` takes only the XML namespace, and `xmlns` (as the name or
/// the prefix) takes only the XMLNS namespace, which takes nothing else.
///
/// The DOM's rule and no more, for elements and attributes, in HTML and XML
/// alike. Namespaces in XML's converse - the XML namespace only under `xml` -
/// is not a naming rule here: rc1 applied it to `set_attribute_ns`, and
/// refused the DOM's `setAttributeNS(XML, "a:bb")`. The XML serializer writes
/// such an attribute as `xml:bb` instead, as DOM Parsing does.
pub fn namespace_fits(ns: &[u8], qname: &[u8], prefix: &[u8]) -> bool {
    use crate::xml::{XMLNS_NS_URI, XML_NS_URI};
    let is_xmlns = qname == b"xmlns" || prefix == b"xmlns";
    if !prefix.is_empty() && ns.is_empty() {
        return false;
    }
    if prefix == b"xml" && ns != XML_NS_URI {
        return false;
    }
    is_xmlns == (ns == XMLNS_NS_URI)
}

/// Check that `qname`, `prefix` and `local` describe one DOM element name -
/// valid under the WHATWG rules, and `qname` exactly `prefix:local` (or
/// `local` when there is no prefix) - and split it, with `local` as a [`Name`].
pub fn split_loose_dom_name<'l>(
    qname: &[u8],
    prefix: Option<&[u8]>,
    local: &'l [u8],
) -> Result<(Split, Name<'l>), LooseNameError> {
    let Some(name) = Name::element(local) else {
        return Err(LooseNameError::Local);
    };
    let Some(p) = prefix else {
        let split = Split::unprefixed(qname.len() as u32);
        if !split.describes(qname, b"", local) {
            return Err(LooseNameError::UnprefixedMismatch);
        }
        return Ok((split, name));
    };
    if !prefix_ok(p) {
        return Err(LooseNameError::Prefix);
    }
    let split = Split::prefixed(p.len() as u32, local.len() as u32);
    if !split.describes(qname, p, local) {
        return Err(LooseNameError::PrefixedMismatch);
    }
    Ok((split, name))
}
