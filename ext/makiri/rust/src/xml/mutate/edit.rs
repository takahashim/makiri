//! Changing what one node IS - its name, its content - as opposed to where it
//! sits (`super::insert`) or what it carries (`super::attr`).

#![forbid(unsafe_code)]

use super::assign_qname;
use super::ns::resolve_ns;
use crate::xml::chars::validate_chars;
use crate::xml::qname::split_checked;
use crate::xml::{
    Document, Link, MutStatus, NodeId, NodeType, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED,
};

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
    let ns = match resolve_ns(doc, scope, name, &sp, is_attr, connected) {
        Ok(ns) => ns,
        Err(st) => return st,
    };
    /* copy the new qname BEFORE writing ns_uri, so an OOM leaves node intact */
    let st = assign_qname(doc, node, name, &sp);
    if st != MutStatus::Ok {
        return st;
    }
    {
        let n = doc.node_mut(node);
        n.ns_uri = ns;
        n.flags &= !FLAG_DOM_LOOSE_NAME;
    }
    /* A rename picks a new prefix, so it decides a new URI from the scope the
     * node is in right now - and that decision is the node's identity from here
     * (an element's; an attribute follows its element). */
    if connected && !is_attr {
        doc.node_mut(node).flags |= FLAG_NS_RESOLVED;
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
            if doc.set_value_bytes(node, text).is_err() {
                return MutStatus::Oom;
            }
            MutStatus::Ok
        }
        Some(NodeType::Element) => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: Option<NodeId> = None;
            if !text.is_empty() {
                let v = match doc.store(text) {
                    Ok(v) => v,
                    Err(_) => return MutStatus::Oom,
                };
                let n = match doc.new_node(NodeType::Text) {
                    Ok(n) => n,
                    Err(_) => return MutStatus::Oom,
                };
                doc.node_mut(n).value = v;
                t = Some(n);
            }
            let mut c = doc.first_child(node);
            while let Some(cur) = c {
                let nx = doc.next(cur);
                {
                    let n = doc.node_mut(cur);
                    n.parent = Link::NONE;
                    n.prev = Link::NONE;
                    n.next = Link::NONE;
                }
                c = nx;
            }
            {
                let n = doc.node_mut(node);
                n.first_child = Link::from_option(t);
                n.last_child = Link::from_option(t);
            }
            if let Some(t) = t {
                doc.set_parent(t, Some(node));
            }
            MutStatus::Ok
        }
        _ => MutStatus::Type,
    }
}

/* ============================ Phase 2: building ============================ */
