//! Setting and removing attributes, by raw qualified name and by `(namespace,
//! local name)` - the DOM's two keys for the same node.
//!
//! Each of the four finds its attribute with one scan ([`find_attr`]), which
//! also finds the list's end - what the tail argument to
//! `Document::link_attr` is for. `set_attribute` scans once more when it
//! ADDS an attribute, for the key-uniqueness check ([`key_taken`]).

#![forbid(unsafe_code)]

use super::assign_qname;
use super::ns::{resolve_ns, Ns, Resolution, Resolved, NO_NS};
use crate::xml::attr_key::{key_taken, AttrKey};
use crate::xml::qname::{ns_decl_check, split_checked, xmlns_prefix, Split};
use crate::xml::{ArenaKind, AttrNs, Document, MutError, NodeFlags, NodeId};

/// Build a fresh ATTRIBUTE (qname + value + namespace) and link it onto `el`
/// after `tail`, the last entry the caller's own scan reached.
fn build_attr(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    sp: &Split,
    val: &[u8],
    ns: Resolved,
    tail: Option<NodeId>,
) -> Result<NodeId, MutError> {
    let attr = doc.new_node(ArenaKind::Attribute)?;
    assign_qname(doc, attr, name, sp)?;
    doc.set_value_bytes(attr, val)?;
    ns.write_attr(doc, attr);
    doc.link_attr(el, tail, attr);
    Ok(attr)
}

/// Whether an attribute named `name` may hold `val`: anything but a namespace
/// declaration the §3 rules forbid ([`ns_decl_check`]), refused as
/// [`MutError::BadNsDecl`] with the clause it broke.
pub(super) fn decl_check(name: &[u8], val: &[u8]) -> Result<(), MutError> {
    match xmlns_prefix(name) {
        Some(p) => ns_decl_check(p, val).map_err(MutError::BadNsDecl),
        None => Ok(()),
    }
}

/// Where the first attribute of `el` that `key` matches sits in its list: the
/// attribute and the one before it, or - when none matches - the last one,
/// which a new attribute is linked after.
enum AttrSlot {
    Found { prev: Option<NodeId>, attr: NodeId },
    Absent { tail: Option<NodeId> },
}

/// Find `el`'s first attribute `key` matches. Read-only: the edit that follows
/// is the caller's, once the walk is over.
fn find_attr(doc: &Document, el: NodeId, key: AttrKey<'_>) -> AttrSlot {
    let mut prev = None;
    for attr in doc.attributes(el) {
        if key.matches(doc, attr) {
            return AttrSlot::Found { prev, attr };
        }
        prev = Some(attr);
    }
    AttrSlot::Absent { tail: prev }
}

pub fn set_attribute(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutError> {
    if doc.type_(el) != Some(ArenaKind::Element) {
        return Err(MutError::Type);
    }
    let sp = split_checked(name).ok_or(MutError::BadName)?;
    /* An attribute with this qualified name gets the value and nothing else,
     * as the DOM's setAttribute does: its namespace is its own, decided when
     * it was named. Re-deriving it here gave a second attribute the key of
     * another (`q:x` moved under a scope where `q` meant another attribute's
     * namespace), silently, and dropped a namespace set_attribute_ns gave. */
    let tail = match find_attr(doc, el, AttrKey::QName(name)) {
        AttrSlot::Found { attr, .. } => {
            set_existing_value(doc, attr, val)?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    decl_check(name, val)?;
    let how = Resolution {
        connected: doc.is_connected(el),
        placed: None,
    };
    let r = resolve_ns(doc, Some(el), name, &sp, true, how)?;
    /* No attribute has this QName, but one may have its key under another
     * prefix for the same URI (p:a beside q:a, both bound to one URI). A
     * pending one has no key yet: the insertion that decides it checks. */
    let local = &name[sp.local_off as usize..];
    if !r.pending && r.ns.len != 0 && key_taken(doc, el, doc.span(r.ns), local, None) {
        return Err(MutError::DuplicateAttr);
    }
    build_attr(doc, el, name, &sp, val, r, tail)
}

/// A new value `val` for the existing attribute `attr` - `[]=` on one it
/// finds, and an Attr's own `content=` - held to the declaration rules when the
/// attribute is named as one, whether or not its current value binds
/// (`set_attribute_ns` may have given it one that does not), since setting it
/// names a declaration to make. A DOM-loose attribute merely named `xmlns`
/// declares nothing, so its value is no URI to check.
///
/// Rebinding a declaration moves no node already decided: a decided URI is a
/// node's identity (`ns`), as with any declaration change.
pub(super) fn set_existing_value(
    doc: &mut Document,
    attr: NodeId,
    val: &[u8],
) -> Result<(), MutError> {
    if let Some(prefix) = doc.declaration_named(attr) {
        ns_decl_check(prefix, val).map_err(MutError::BadNsDecl)?;
    }
    doc.set_value_bytes(attr, val)?;
    Ok(())
}

/// The DOM's `setAttribute` on a non-HTML element, name for name: the first
/// attribute whose qualified name is `name` gets the value; with none, a new
/// one in NO namespace whose local name is the whole of `name`, colons and all.
///
/// XML has no such attribute when `name` is not an NCName or is `xmlns` -
/// written out, `xlink:href` in no namespace names an unbound prefix and
/// `xmlns` a declaration - so that one is marked DOM-loose: it stays an
/// attribute and not a declaration (`Document::decl_prefix`), and the
/// serializers refuse it, as they refuse a DOM-loose element. Any other name
/// makes the plain attribute `set_attribute_ns(nil, name)` would.
///
/// `name` is held to the DOM's "valid attribute local name" only, and the
/// value to nothing: a value XML cannot write is refused by the serializers,
/// as every attribute value is.
pub fn set_loose_dom_attribute(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutError> {
    if doc.type_(el) != Some(ArenaKind::Element) {
        return Err(MutError::Type);
    }
    if !crate::xml::dom_name::valid_attribute_local_name(name) {
        return Err(MutError::BadDomName("invalid DOM attribute name"));
    }
    let Ok(len) = u32::try_from(name.len()) else {
        return Err(MutError::BadName);
    };
    /* The DOM's setAttribute checks no value: a declaration given one it
     * cannot hold (`xmlns:p=""`) binds nothing from then on
     * (`Document::decl_prefix`), as `set_attribute_ns` leaves one. */
    let tail = match find_attr(doc, el, AttrKey::QName(name)) {
        AttrSlot::Found { attr, .. } => {
            doc.set_value_bytes(attr, val)?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    let loose = name == b"xmlns" || split_checked(name).is_none_or(|sp| sp.prefix_len != 0);
    let sp = Split::unprefixed(len);
    let attr = build_attr(doc, el, name, &sp, val, Resolved::decided(NO_NS), tail)?;
    let n = doc.node_mut(attr);
    n.attr_ns = AttrNs::Explicit;
    n.flags.set(NodeFlags::DOM_LOOSE_NAME, loose);
    Ok(attr)
}

/// Remove `el`'s attribute named `name`; `true` when one was removed.
pub fn remove_attribute(doc: &mut Document, el: NodeId, name: &[u8]) -> bool {
    remove_attr_by(doc, el, AttrKey::QName(name))
}

/// Unlink `el`'s attribute that `key` matches; `true` when there was one. The
/// shared body of the two removers, which differ only in the key.
fn remove_attr_by(doc: &mut Document, el: NodeId, key: AttrKey<'_>) -> bool {
    if doc.type_(el) != Some(ArenaKind::Element) {
        return false;
    }
    match find_attr(doc, el, key) {
        AttrSlot::Found { prev, attr } => {
            doc.unlink_attr(el, prev, attr);
            true
        }
        AttrSlot::Absent { .. } => false,
    }
}

/// The DOM's `setAttributeNS`: the attribute keyed by (`ns`, local name) gets
/// `val`, or a new one is made with `ns` as its own namespace.
///
/// A declaration Namespaces in XML §3 forbids - `xmlns:p=""` above all, which
/// the DOM makes (WPT `XMLSerializer-serializeToString.html`) - is not refused
/// but held: an attribute in the XMLNS namespace that binds nothing
/// (`Document::decl_prefix` decides from its value, so a later allowed value
/// makes it a declaration again) and that the serializers refuse. `[]=`
/// ([`set_attribute`]) still refuses one: it names a declaration to make.
///
/// A name the DOM takes and XML cannot write - `p:a}b`, whose local name is
/// no NCName - is split by the DOM's rule instead
/// ([`crate::xml::dom_name::validate_and_extract`], as HTML's
/// `set_attribute_ns` splits every name) and made DOM-loose, so the
/// serializers refuse it. It was refused as no XML name, where the DOM's
/// setAttributeNS succeeds.
pub fn set_attribute_ns(
    doc: &mut Document,
    el: NodeId,
    ns: &[u8],
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutError> {
    if doc.type_(el) != Some(ArenaKind::Element) {
        return Err(MutError::Type);
    }
    let (sp, loose) = match split_checked(name) {
        Some(sp) => {
            let prefix = &name[..sp.prefix_len as usize];
            if !crate::xml::dom_name::namespace_fits(ns, name, prefix) {
                return Err(MutError::BadNsName);
            }
            (sp, false)
        }
        None => (dom_split(ns, name)?, true),
    };
    let local = &name[sp.local_off as usize..];
    let tail = match find_attr(doc, el, AttrKey::Ns { ns, local }) {
        AttrSlot::Found { attr, .. } => {
            /* Whether it binds is read from its OWN name and the new value
             * (`Document::decl_prefix`): `xmlns:xmlns` finds the default
             * declaration `xmlns` (both are XMLNS + `xmlns`). */
            doc.set_value_bytes(attr, val)?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() { NO_NS } else { doc.store(ns)? };
    let attr = build_attr(doc, el, name, &sp, val, Resolved::decided(nsv), tail)?;
    let n = doc.node_mut(attr);
    n.attr_ns = AttrNs::Explicit;
    n.flags.set(NodeFlags::DOM_LOOSE_NAME, loose);
    Ok(attr)
}

/// `name` split by the DOM's "validate and extract" for an attribute in `ns`,
/// for one that is no XML QName: [`MutError::BadDomName`] when the DOM refuses
/// the name, [`MutError::BadNsName`] when the namespace does not fit it.
fn dom_split(ns: &[u8], name: &[u8]) -> Result<Split, MutError> {
    use crate::xml::dom_name::{valid_attribute_local_name, validate_and_extract, ExtractError};
    let (prefix, local) =
        validate_and_extract(ns, name, valid_attribute_local_name).map_err(|e| match e {
            ExtractError::Name => MutError::BadDomName("invalid DOM attribute name"),
            ExtractError::Namespace => MutError::BadNsName,
        })?;
    let (Ok(p), Ok(l)) = (u32::try_from(prefix.len()), u32::try_from(local.len())) else {
        return Err(MutError::BadName);
    };
    /* `validate_and_extract` splits at the first colon, and an empty prefix
     * is no valid one: a name with a colon has a prefix here. */
    Ok(if prefix.is_empty() {
        Split::unprefixed(l)
    } else {
        Split::prefixed(p, l)
    })
}

/// Remove `el`'s attribute keyed by `(ns, local)`; `true` when one was removed.
pub fn remove_attribute_ns(doc: &mut Document, el: NodeId, ns: &[u8], local: &[u8]) -> bool {
    remove_attr_by(doc, el, AttrKey::Ns { ns, local })
}
