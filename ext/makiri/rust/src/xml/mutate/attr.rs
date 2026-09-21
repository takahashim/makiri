//! Setting and removing attributes, by raw qualified name and by `(namespace,
//! local name)` - the DOM's two keys for the same node.
//!
//! Each of the four walks the element's attribute list exactly once: the scan
//! that looks for a match also finds the list's end, which is what the tail
//! argument to `Document::link_attr` is for.

#![forbid(unsafe_code)]

use super::ns::{resolve_ns, Ns, NO_NS};
use super::{arena, assign_qname};
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, Split};
use crate::xml::{Document, MutStatus, NodeId, NodeType};

/// Build a fresh ATTRIBUTE (qname + value + namespace) and link it onto `el`
/// after `tail`, the last entry the caller's own scan reached.
fn build_attr(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    sp: &Split,
    val: &[u8],
    ns: Ns,
    tail: Option<NodeId>,
) -> Result<NodeId, MutStatus> {
    let attr = arena(doc.new_node(NodeType::Attribute))?;
    let st = assign_qname(doc, attr, name, sp);
    if st != MutStatus::Ok {
        return Err(st);
    }
    arena(doc.set_value_bytes(attr, val))?;
    doc.node_mut(attr).ns_uri = ns;
    doc.link_attr(el, tail, attr);
    Ok(attr)
}

pub fn set_attribute(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutStatus> {
    if doc.type_(el) != Some(NodeType::Element) {
        return Err(MutStatus::Type);
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutStatus::BadName),
    };
    /* xmlns:foo="" must not bind a prefix to the empty namespace */
    if val.is_empty() && sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MutStatus::BadNsDecl);
    }
    if !val.is_empty() && !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let connected = doc.is_connected(el);
    let ns = resolve_ns(doc, Some(el), name, &sp, true, connected)?;
    /* an existing attribute with the same raw QName -> replace its value */
    let mut tail = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if doc.qname(attr) == name {
            arena(doc.set_value_bytes(attr, val))?;
            doc.node_mut(attr).ns_uri = ns;
            return Ok(attr);
        }
        tail = Some(attr);
        a = doc.next(attr);
    }
    build_attr(doc, el, name, &sp, val, ns, tail)
}

/// Remove `el`'s attribute named `name`; `true` when one was removed.
pub fn remove_attribute(doc: &mut Document, el: NodeId, name: &[u8]) -> bool {
    if doc.type_(el) != Some(NodeType::Element) {
        return false;
    }
    let mut prev: Option<NodeId> = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if doc.qname(attr) == name {
            doc.unlink_attr(el, prev, attr);
            return true;
        }
        prev = Some(attr);
        a = doc.next(attr);
    }
    false
}

/// `a` is keyed by (ns, local) - the DOM key; an empty wanted namespace
/// matches an attribute with no namespace.
fn attr_matches_ns(doc: &Document, a: NodeId, ns: &[u8], local: &[u8]) -> bool {
    doc.node(a).ns_uri.len as usize == ns.len()
        && (ns.is_empty() || doc.ns(a) == ns)
        && doc.local(a) == local
}

pub fn set_attribute_ns(
    doc: &mut Document,
    el: NodeId,
    ns: &[u8],
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutStatus> {
    if doc.type_(el) != Some(NodeType::Element) {
        return Err(MutStatus::Type);
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutStatus::BadName),
    };
    if !val.is_empty() && !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let local = &name[sp.local_off as usize..];
    let mut tail = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if attr_matches_ns(doc, attr, ns, local) {
            arena(doc.set_value_bytes(attr, val))?;
            return Ok(attr);
        }
        tail = Some(attr);
        a = doc.next(attr);
    }
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        arena(doc.store(ns))?
    };
    build_attr(doc, el, name, &sp, val, nsv, tail)
}

/// Remove `el`'s attribute keyed by `(ns, local)`; `true` when one was removed.
pub fn remove_attribute_ns(doc: &mut Document, el: NodeId, ns: &[u8], local: &[u8]) -> bool {
    if doc.type_(el) != Some(NodeType::Element) {
        return false;
    }
    let mut prev: Option<NodeId> = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if attr_matches_ns(doc, attr, ns, local) {
            doc.unlink_attr(el, prev, attr);
            return true;
        }
        prev = Some(attr);
        a = doc.next(attr);
    }
    false
}
