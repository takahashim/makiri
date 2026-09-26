//! Setting and removing attributes, by raw qualified name and by `(namespace,
//! local name)` - the DOM's two keys for the same node.
//!
//! Each of the four finds its attribute with one scan ([`find_attr`]), which
//! also finds the list's end - what the tail argument to
//! `Document::link_attr` is for. `set_attribute` scans once more when it
//! ADDS an attribute, for the key-uniqueness check ([`key_taken`]).

#![forbid(unsafe_code)]

use super::assign_qname;
use super::ns::{resolve_ns, Ns, Resolved, NO_NS};
use crate::xml::attr_key::{key_taken, AttrKey};
use crate::xml::chars::validate_chars;
use crate::xml::qname::{ns_decl_check, split_checked, xmlns_prefix, Split};
use crate::xml::{ArenaKind, AttrNs, Document, MutError, NodeId};

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
    decl_check(name, val)?;
    if !validate_chars(val) {
        return Err(MutError::BadChars);
    }
    /* An attribute with this qualified name gets the value and nothing else,
     * as the DOM's setAttribute does: its namespace is its own, decided when
     * it was named. Re-deriving it here gave a second attribute the key of
     * another (`q:x` moved under a scope where `q` meant another attribute's
     * namespace), silently, and dropped a namespace set_attribute_ns gave. */
    let tail = match find_attr(doc, el, AttrKey::QName(name)) {
        AttrSlot::Found { attr, .. } => {
            doc.set_value_bytes(attr, val)?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    let connected = doc.is_connected(el);
    let r = resolve_ns(doc, Some(el), name, &sp, true, connected)?;
    /* No attribute has this QName, but one may have its key under another
     * prefix for the same URI (p:a beside q:a, both bound to one URI). A
     * pending one has no key yet: the insertion that decides it checks. */
    let local = &name[sp.local_off as usize..];
    if !r.pending && r.ns.len != 0 && key_taken(doc, el, doc.span(r.ns), local, None) {
        return Err(MutError::DuplicateAttr);
    }
    build_attr(doc, el, name, &sp, val, r, tail)
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
    let sp = split_checked(name).ok_or(MutError::BadName)?;
    if !crate::xml::qname::ns_fits_name(ns, name, &sp) {
        return Err(MutError::BadNsName);
    }
    decl_check(name, val)?;
    if !validate_chars(val) {
        return Err(MutError::BadChars);
    }
    let local = &name[sp.local_off as usize..];
    let tail = match find_attr(doc, el, AttrKey::Ns { ns, local }) {
        AttrSlot::Found { attr, .. } => {
            doc.set_value_bytes(attr, val)?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() { NO_NS } else { doc.store(ns)? };
    let attr = build_attr(doc, el, name, &sp, val, Resolved::decided(nsv), tail)?;
    doc.node_mut(attr).attr_ns = AttrNs::Explicit;
    Ok(attr)
}

/// Remove `el`'s attribute keyed by `(ns, local)`; `true` when one was removed.
pub fn remove_attribute_ns(doc: &mut Document, el: NodeId, ns: &[u8], local: &[u8]) -> bool {
    remove_attr_by(doc, el, AttrKey::Ns { ns, local })
}
