//! Reading the namespace declarations in scope - rules over the tree as it
//! stands, which change nothing.
//!
//! The mutators resolve names against them (`mutate::ns`), and the XML
//! serializer leaves out the one declaration they ignore
//! ([`ignored_default_decl`]); neither owns the rule, so it lives here, where
//! both read it. Canonical XML reads the same declaration differently ON
//! PURPOSE: it renders a document's own declarations rather than planning
//! them, so it refuses one that contradicts its element
//! (`Failure::NamespaceMismatch`) instead of leaving it out.

#![forbid(unsafe_code)]

use crate::xml::{ArenaKind, Document, NodeId, Span};

/// A subtree about to be placed under `context`, read as if it already were:
/// past `root`, its ancestors are `context` and those above it, whatever
/// `root`'s own parent is. What lets a placement plan its names over the tree
/// as it stands, before anything is linked.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub(crate) root: NodeId,
    pub(crate) context: NodeId,
}

/// An `xmlns="X"` attribute (X non-empty) on an unprefixed element DECIDED to
/// be in no namespace - a declaration that contradicts its own element, which
/// `root["xmlns"] = "urn:x"` makes. It is ignored, by the serializer and the
/// mutators alike.
///
/// Written, it would put the element in X on re-parse; planning a prefix for
/// the element instead gave `xmlns:ns1=""`, which Namespaces 1.0 forbids. The
/// DOM Parsing and Serialization spec ignores such a declaration and writes
/// `xmlns=""` where an inherited default would otherwise claim the element.
/// Only the serializer did, so a later rename (since removed) or a new child
/// still resolved against it - `e.name = "e"` moved `e` into X. Now [`resolve_in_scope`] skips
/// it as well. (Nokogiri writes the attribute, and the element moves.)
///
/// Only a DECIDED no-namespace: an unresolved element's empty URI means "not
/// decided yet", and its own declaration is what decides it.
pub(crate) fn ignored_default_decl(doc: &Document, el: NodeId) -> Option<NodeId> {
    if !doc.prefix(el).is_empty()
        || !doc.ns(el).is_empty()
        || doc.is_loose_name(el)
        || !doc.element_ns_decided(el)
    {
        return None;
    }
    for at in doc.attributes(el) {
        if doc.decl_prefix(at) == Some(&b""[..]) {
            return (doc.node(at).value.len != 0).then_some(at);
        }
    }
    None
}

/// What `prefix` ("" = default) is bound to at or above `node`, by the
/// declarations the mutators resolve against; empty when unbound. The
/// resolver's own lookup, exposed to the selftest that pins it; nothing else
/// asks.
#[cfg(test)]
pub(crate) fn namespace_in_scope<'d>(doc: &'d Document, node: NodeId, prefix: &[u8]) -> &'d [u8] {
    doc.span(resolve_in_scope(doc, Some(node), prefix, None))
}

/// Nearest in-scope binding for `prefix` ("" = default) at or above `node` -
/// above a [`Placement`]'s root, at or above its context;
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
pub(super) fn resolve_in_scope(
    doc: &Document,
    node: Option<NodeId>,
    prefix: &[u8],
    placed: Option<Placement>,
) -> Span {
    /* The walk upward, through `placed`'s context past its root. */
    let up = |id: NodeId| match placed {
        Some(p) if p.root == id => Some(p.context),
        _ => doc.parent(id),
    };
    let mut e = node;
    while let Some(id) = e {
        if doc.type_(id) == Some(ArenaKind::Element) {
            let ignored = ignored_default_decl(doc, id);
            for at in doc.attributes(id) {
                if let Some(p) = doc.decl_prefix(at) {
                    if p == prefix && Some(at) != ignored {
                        return doc.try_node(at).map_or(Span::EMPTY, |n| n.value);
                    }
                }
            }
        }
        e = up(id);
    }
    Span::EMPTY
}
