//! Changing what one node IS - its name, its content - as opposed to where it
//! sits (`super::insert`) or what it carries (`super::attr`).

#![forbid(unsafe_code)]

use super::ns::resolve_ns;
use super::{arena, assign_qname};
use crate::xml::chars::validate_chars;
use crate::xml::qname::split_checked;
use crate::xml::{Document, MutStatus, NodeId, NodeType, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED};

/// Whether `text` is free of the SEQUENCE its node kind cannot hold: "--" (or
/// a trailing "-") in a comment, "]]>" in CDATA, "?>" in a PI. Each would close
/// the construct early, so the value is refused rather than escaped.
///
/// A mutation precondition, not a naming rule: the parser never needs it,
/// because it finds those sequences structurally while scanning.
pub(super) fn value_seq_ok(node_type: NodeType, text: &[u8]) -> bool {
    match node_type {
        NodeType::Comment => text.last() != Some(&b'-') && !text.windows(2).any(|w| w == b"--"),
        NodeType::CData => !text.windows(3).any(|w| w == b"]]>"),
        NodeType::Pi => !text.windows(2).any(|w| w == b"?>"),
        _ => true,
    }
}

pub fn rename(doc: &mut Document, node: NodeId, name: &[u8]) -> MutStatus {
    if doc.type_(node) != Some(NodeType::Element) && doc.type_(node) != Some(NodeType::Attribute) {
        return MutStatus::Type;
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MutStatus::BadName,
    };
    let is_attr = doc.type_(node) == Some(NodeType::Attribute);
    let scope: Option<NodeId> = if is_attr {
        doc.parent(node)
    } else {
        Some(node)
    };
    let connected = scope.is_some_and(|s| doc.is_connected(s));
    let r = match resolve_ns(doc, scope, name, &sp, is_attr, connected) {
        Ok(r) => r,
        Err(st) => return st,
    };
    /* An attribute renamed into a declaration must be one its value allows, and
     * onto another attribute's key is a second attribute with that key - both
     * rules the parser holds a document to (§3). */
    if let (true, Some(el)) = (is_attr, scope) {
        if let Err(st) = super::attr::decl_check(name, doc.value(node)) {
            return st;
        }
        let local = &name[sp.local_off as usize..];
        if !r.pending && super::attr::key_taken(doc, el, doc.span(r.ns), local, Some(node)) {
            return MutStatus::DuplicateAttr;
        }
    }
    /* copy the new qname BEFORE writing ns_uri, so an OOM leaves node intact */
    let st = assign_qname(doc, node, name, &sp);
    if st != MutStatus::Ok {
        return st;
    }
    doc.node_mut(node).flags &= !FLAG_DOM_LOOSE_NAME;
    if is_attr {
        r.write_attr(doc, node);
        return MutStatus::Ok;
    }
    /* A rename picks a new prefix, so it decides a new URI from the scope the
     * node is in right now - and that decision is the node's identity from
     * here. Detached, there is no such scope: the element is undecided again,
     * so the insertion that connects it resolves the new name (it kept the old
     * decision, and `q:n` came back bound to ""). */
    let n = doc.node_mut(node);
    n.ns_uri = r.ns;
    if connected {
        n.flags |= FLAG_NS_RESOLVED;
    } else {
        n.flags &= !FLAG_NS_RESOLVED;
    }
    MutStatus::Ok
}

pub fn set_content(doc: &mut Document, node: NodeId, text: &[u8]) -> MutStatus {
    if !text.is_empty() && !validate_chars(text) {
        return MutStatus::BadChars;
    }
    match doc.type_(node) {
        Some(ty @ (NodeType::Text | NodeType::CData | NodeType::Comment | NodeType::Pi)) => {
            if !value_seq_ok(ty, text) {
                return MutStatus::BadChars;
            }
            match arena(doc.set_value_bytes(node, text)) {
                Ok(()) => MutStatus::Ok,
                Err(st) => st,
            }
        }
        Some(NodeType::Element) => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: Option<NodeId> = None;
            if !text.is_empty() {
                let v = match arena(doc.store(text)) {
                    Ok(v) => v,
                    Err(st) => return st,
                };
                let n = match arena(doc.new_node(NodeType::Text)) {
                    Ok(n) => n,
                    Err(st) => return st,
                };
                doc.node_mut(n).value = v;
                t = Some(n);
            }
            doc.replace_children(node, t);
            MutStatus::Ok
        }
        _ => MutStatus::Type,
    }
}
