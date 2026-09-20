//! Deciding an element's or attribute's namespace URI.
//!
//! The rules mirror the parser's (§7): a prefix resolves against the in-scope
//! `xmlns` declarations at or above the node. What differs is WHEN it is an
//! error - inside a still-detached subtree an unbound prefix is deferred, not
//! refused, so a subtree built bottom-up and then attached gives the same tree
//! as one built top-down.
//!
//! A decided URI is the node's IDENTITY from then on (`FLAG_NS_RESOLVED`): moving
//! the node does not change it, and the serializer emits whatever declarations
//! the output needs to reproduce it. So resolution happens exactly once per
//! element, and [`resolve_subtree`] is all-or-nothing - one pass that only
//! computes, and, only if every prefix binds, a second that writes.

#![forbid(unsafe_code)]

use super::copy_span;
use crate::xml::qname::{xmlns_prefix, Split};
use crate::xml::{
    Document, MutStatus, NodeId, NodeType, Span, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED,
};

/// A resolved namespace: a byte-store span (empty = no namespace).
pub(super) type Ns = Span;

pub(super) const NO_NS: Ns = Span::EMPTY;

/// Resolve `name` (split per `sp`) applied at `scope` (mirrors the parser's §7
/// rules). An unbound prefix is an error only when connected; deferred
/// (unresolved) otherwise.
pub(super) fn resolve_ns(
    doc: &Document,
    scope: Option<NodeId>,
    name: &[u8],
    sp: &Split,
    is_attr: bool,
    connected: bool,
) -> Result<Ns, MutStatus> {
    let prefix = &name[..sp.prefix_len as usize];
    if is_attr && xmlns_prefix(name).is_some() {
        return Ok(doc.xmlns_ns_span());
    }
    if sp.prefix_len == 0 {
        if is_attr {
            return Ok(NO_NS); /* unprefixed attribute -> no namespace */
        }
        let s = resolve_in_scope(doc, scope, b"");
        return Ok(if s.len > 0 { s } else { NO_NS });
    }
    if prefix == b"xml" {
        return Ok(doc.xml_ns_span());
    }
    if prefix == b"xmlns" {
        return Err(MutStatus::BadName);
    }
    let s = resolve_in_scope(doc, scope, prefix);
    if s.len > 0 {
        Ok(s)
    } else if connected {
        Err(MutStatus::UnboundNs)
    } else {
        Ok(NO_NS)
    }
}

/// Resolve the namespace of element `e` and its attributes.
///
/// `commit` selects the pass: false only computes (to find out whether every
/// prefix in the subtree binds), true writes the resolved URIs.
fn resolve_node_ns(doc: &mut Document, e: NodeId, connected: bool, commit: bool) -> MutStatus {
    if doc.node(e).flags & FLAG_DOM_LOOSE_NAME == 0 {
        let name = match copy_span(doc.qname(e)) {
            Ok(v) => v,
            Err(st) => return st,
        };
        let sp = doc.split_of(e);
        match resolve_ns(doc, Some(e), &name, &sp, false, connected) {
            Ok(ns) => {
                if commit {
                    doc.node_mut(e).ns_uri = ns
                }
            }
            Err(st) => return st,
        }
    }
    let mut a = doc.attrs(e);
    while let Some(attr) = a {
        let name = match copy_span(doc.qname(attr)) {
            Ok(v) => v,
            Err(st) => return st,
        };
        let sp = doc.split_of(attr);
        match resolve_ns(doc, Some(e), &name, &sp, true, connected) {
            Ok(ns) => {
                if commit {
                    doc.node_mut(attr).ns_uri = ns
                }
            }
            Err(st) => return st,
        }
        a = doc.next(attr);
    }
    /* Only mark once connected: resolution inside a still-detached fragment is
     * deferred (an unbound prefix is not an error there), so the node must stay
     * open to being resolved again when the fragment joins the document. */
    if commit && connected {
        doc.node_mut(e).flags |= FLAG_NS_RESOLVED;
    }
    MutStatus::Ok
}

/// True once `e`'s namespace has been decided - by the parser, or by resolving
/// it against the context it was first inserted into.
fn ns_is_decided(doc: &Document, e: NodeId) -> bool {
    doc.node(e).flags & FLAG_NS_RESOLVED != 0
}

/// Re-resolve every element in `root`'s subtree, all-or-nothing: one pass that
/// only computes, and - only if every prefix binds - a second that writes.
fn resolve_subtree(doc: &mut Document, root: NodeId, connected: bool) -> MutStatus {
    for commit in [false, true] {
        let mut cur = Some(root);
        while let Some(c) = cur {
            if doc.type_(c) == Some(NodeType::Element) && !ns_is_decided(doc, c) {
                let st = resolve_node_ns(doc, c, connected, commit);
                if st != MutStatus::Ok {
                    return st; /* commit == false: nothing written yet */
                }
            }
            cur = doc.preorder_next(root, c);
        }
    }
    MutStatus::Ok
}

/// Resolve `node`'s subtree as if it were a child of `context`, WITHOUT linking
/// it (borrow node.parent for the ancestor walk, then restore).
pub(super) fn resolve_into(doc: &mut Document, node: NodeId, context: NodeId) -> MutStatus {
    let saved = doc.parent(node);
    doc.set_parent(node, Some(context));
    let st = resolve_subtree(doc, node, doc.is_connected(node));
    doc.set_parent(node, saved);
    st
}

/* ---- copying ----
 *
 * A copy READS one arena and WRITES another - or, for `clone_node`, the same
 * one, which Rust cannot express as `(&mut Document, &Document)`. Rather than
 * keep two copies of every routine (one per source), a copy lifts the node's
 * fields OUT of the source first (`CopiedNode::read`) and writes them back
 * (`CopiedNode::write`). The fields had to be owned anyway - a `&mut Document`
 * cannot be held across a read of its own byte store - so the split costs
 * nothing and leaves one body per operation. */

/// Nearest in-scope binding for `prefix` ("" = default) at or above `node`;
/// [`Span::EMPTY`] when there is none, which callers treat like an empty
/// binding. Not an `Option<Span>`: `None` leaves the payload undefined, and LLVM
/// folds the caller's `Some(s) if s.len > 0` into one branch that reads it -
/// harmless, but Valgrind reports it as an uninitialised-value jump.
///
/// A free function here rather than a `Document` method in `arena`: walking the
/// ancestors for an `xmlns` declaration is a NAMESPACE rule, and the arena
/// stores nodes rather than interpreting them. Moving it also made it go through
/// the CHECKED accessors, which is the right thing at this layer - it used to
/// index links raw, which only the arena's own private accessors may do.
fn resolve_in_scope(doc: &Document, node: Option<NodeId>, prefix: &[u8]) -> Span {
    let mut e = node;
    while let Some(id) = e {
        if doc.type_(id) == Some(NodeType::Element) {
            let mut a = doc.attrs(id);
            while let Some(at) = a {
                if let Some(p) = xmlns_prefix(doc.qname(at)) {
                    if p == prefix {
                        return doc.try_node(at).map_or(Span::EMPTY, |n| n.value);
                    }
                }
                a = doc.next(at);
            }
        }
        e = doc.parent(id);
    }
    Span::EMPTY
}
