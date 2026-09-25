//! Deciding an element's or attribute's namespace URI.
//!
//! The rules mirror the parser's (§7): a prefix resolves against the in-scope
//! `xmlns` declarations at or above the node. What differs is WHEN it is an
//! error - inside a still-detached subtree an unbound prefix is deferred, not
//! refused, so a subtree built bottom-up and then attached gives the same tree
//! as one built top-down.
//!
//! A decided URI is the node's IDENTITY from then on (`NodeFlags::NS_RESOLVED`): moving
//! the node does not change it, and the serializer emits whatever declarations
//! the output needs to reproduce it. So resolution happens exactly once per
//! element, and [`resolve_subtree`] is all-or-nothing - one pass that only
//! computes, and, only if every prefix binds, a second that writes.

#![forbid(unsafe_code)]

use crate::falloc::VecPush;
use crate::xml::qname::{xmlns_prefix, Split};
use crate::xml::{ArenaKind, Document, MutError, NodeFlags, NodeId, Span};

/// A resolved namespace: a byte-store span (empty = no namespace).
pub(super) type Ns = Span;

pub(super) const NO_NS: Ns = Span::EMPTY;

/// A resolved name: its namespace, and whether that is still PENDING - a
/// prefix unbound on a detached node, deferred rather than refused.
#[derive(Clone, Copy)]
pub(super) struct Resolved {
    pub ns: Ns,
    pub pending: bool,
}

impl Resolved {
    pub(super) fn decided(ns: Ns) -> Resolved {
        Resolved { ns, pending: false }
    }

    /// Record the outcome on attribute `attr`.
    pub(super) fn write_attr(self, doc: &mut Document, attr: NodeId) {
        let n = doc.node_mut(attr);
        n.ns_uri = self.ns;
        n.flags.remove(NodeFlags::NS_EXPLICIT);
        n.flags.set(NodeFlags::NS_PENDING, self.pending);
    }
}

/// Resolve `name` (split per `sp`) applied at `scope` (mirrors the parser's §7
/// rules). An unbound prefix is an error only when connected; deferred - and
/// reported pending - otherwise.
pub(super) fn resolve_ns(
    doc: &Document,
    scope: Option<NodeId>,
    name: &[u8],
    sp: &Split,
    is_attr: bool,
    connected: bool,
) -> Result<Resolved, MutError> {
    let prefix = &name[..sp.prefix_len as usize];
    if is_attr && xmlns_prefix(name).is_some() {
        return Ok(Resolved::decided(doc.xmlns_ns_span()));
    }
    if sp.prefix_len == 0 {
        if is_attr {
            return Ok(Resolved::decided(NO_NS)); /* unprefixed attribute -> no namespace */
        }
        let s = resolve_in_scope(doc, scope, b"");
        return Ok(Resolved::decided(if s.len > 0 { s } else { NO_NS }));
    }
    if prefix == b"xml" {
        return Ok(Resolved::decided(doc.xml_ns_span()));
    }
    if prefix == b"xmlns" {
        return Err(MutError::BadName);
    }
    let s = resolve_in_scope(doc, scope, prefix);
    if s.len > 0 {
        Ok(Resolved::decided(s))
    } else if connected {
        Err(MutError::UnboundNs)
    } else {
        Ok(Resolved {
            ns: NO_NS,
            pending: true,
        })
    }
}

/// Which pass of an all-or-nothing resolution this is: one that only computes
/// ([`check_node_ns`]: does every prefix in the subtree bind, is every key
/// unique), then one that writes ([`commit_node_ns`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Check,
    Commit,
}

/// What of an element to resolve: its name and every attribute, or - for an
/// element whose own namespace is already decided - only the attributes still
/// pending.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Whole,
    PendingAttrs,
}

/// Whether attribute `attr`'s namespace is (re-)derived from its prefix for
/// this `part`. A namespace given with `set_attribute_ns` is the attribute's
/// own and is never derived again; everything else is, unless only the
/// pending ones are being looked at.
fn rederives(doc: &Document, attr: NodeId, part: Part) -> bool {
    let flags = doc.node(attr).flags;
    !flags.contains(NodeFlags::NS_EXPLICIT)
        && (part == Part::Whole || flags.contains(NodeFlags::NS_PENDING))
}

/// Whether `e`'s own name is resolved for this `part`.
fn resolves_name(doc: &Document, e: NodeId, part: Part) -> bool {
    part == Part::Whole && !doc.node(e).flags.contains(NodeFlags::DOM_LOOSE_NAME)
}

/// The [`Pass::Check`] half for element `e` - see [`Part`]: that every prefix
/// binds, and that its attributes' keys stay unique, the rule the parser holds
/// a document to (§3). Takes `&Document`, so the pass that must write nothing
/// cannot.
fn check_node_ns(doc: &Document, e: NodeId, connected: bool, part: Part) -> Result<(), MutError> {
    if resolves_name(doc, e, part) {
        resolve_ns(
            doc,
            Some(e),
            doc.qname(e),
            &doc.split_of(e),
            false,
            connected,
        )?;
    }
    /* Every attribute's key as it will stand - a re-resolved one's new
     * namespace, anyone else's stored one - leaving out those still pending,
     * which have no namespace to compare yet. */
    let mut keys: Vec<(Span, NodeId)> = Vec::new();
    for attr in doc.attributes(e) {
        let key = if rederives(doc, attr, part) {
            let r = resolve_ns(
                doc,
                Some(e),
                doc.qname(attr),
                &doc.split_of(attr),
                true,
                connected,
            )?;
            (!r.pending).then_some(r.ns)
        } else {
            Some(doc.node(attr).ns_uri)
        };
        if let Some(ns) = key {
            keys.falloc_push((ns, attr)).map_err(|()| MutError::Oom)?;
        }
    }
    if super::attr::keys_repeat(doc, &mut keys) {
        return Err(MutError::DuplicateAttr);
    }
    Ok(())
}

/// The [`Pass::Commit`] half for element `e`: write what [`check_node_ns`]
/// found resolvable. An `Err` here would mean the check let through what the
/// commit refuses; the two resolve the same names against the same scope.
fn commit_node_ns(
    doc: &mut Document,
    e: NodeId,
    connected: bool,
    part: Part,
) -> Result<(), MutError> {
    if resolves_name(doc, e, part) {
        let r = resolve_ns(
            doc,
            Some(e),
            doc.qname(e),
            &doc.split_of(e),
            false,
            connected,
        )?;
        doc.node_mut(e).ns_uri = r.ns;
    }
    /* A cursor, not `attributes()`: the body writes. */
    let mut a = doc.first_attr(e);
    while let Some(attr) = a {
        if rederives(doc, attr, part) {
            let r = resolve_ns(
                doc,
                Some(e),
                doc.qname(attr),
                &doc.split_of(attr),
                true,
                connected,
            )?;
            r.write_attr(doc, attr);
        }
        a = doc.next(attr);
    }
    /* Only mark once connected: resolution inside a still-detached fragment is
     * deferred (an unbound prefix is not an error there), so the node must stay
     * open to being resolved again when the fragment joins the document. */
    if connected {
        doc.node_mut(e).flags.insert(NodeFlags::NS_RESOLVED);
    }
    Ok(())
}

/// Whether any attribute of `e` still has a pending namespace.
fn has_pending_attr(doc: &Document, e: NodeId) -> bool {
    for attr in doc.attributes(e) {
        if doc.node(attr).flags.contains(NodeFlags::NS_PENDING) {
            return true;
        }
    }
    false
}

/// True once `e`'s namespace has been decided - by the parser, or by resolving
/// it against the context it was first inserted into.
fn ns_is_decided(doc: &Document, e: NodeId) -> bool {
    doc.node(e).flags.contains(NodeFlags::NS_RESOLVED)
}

/// Re-resolve every element in `root`'s subtree, all-or-nothing: one pass that
/// only computes, and - only if every prefix binds - a second that writes.
fn resolve_subtree(doc: &mut Document, root: NodeId, connected: bool) -> Result<(), MutError> {
    for pass in [Pass::Check, Pass::Commit] {
        let mut cur = Some(root);
        while let Some(c) = cur {
            if doc.type_(c) == Some(ArenaKind::Element) {
                /* A decided element keeps its own namespace; its attributes set
                 * while it was detached may still be pending. */
                let decided = ns_is_decided(doc, c);
                if !decided || has_pending_attr(doc, c) {
                    /* an Err here comes from the Check pass: nothing written yet */
                    let part = if decided {
                        Part::PendingAttrs
                    } else {
                        Part::Whole
                    };
                    match pass {
                        Pass::Check => check_node_ns(doc, c, connected, part)?,
                        Pass::Commit => commit_node_ns(doc, c, connected, part)?,
                    }
                }
            }
            cur = doc.preorder_next(root, c);
        }
    }
    Ok(())
}

/// Resolve `node`'s subtree as if it were a child of `context`, WITHOUT linking
/// it (borrow node.parent for the ancestor walk, then restore).
pub(super) fn resolve_into(
    doc: &mut Document,
    node: NodeId,
    context: NodeId,
) -> Result<(), MutError> {
    let saved = doc.parent(node);
    doc.set_parent(node, Some(context));
    let st = resolve_subtree(doc, node, doc.is_connected(node));
    doc.set_parent(node, saved);
    st
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
pub fn ignored_default_decl(doc: &Document, el: NodeId) -> Option<NodeId> {
    let node = doc.node(el);
    if node.prefix.len != 0
        || node.ns_uri.len != 0
        || node.flags.contains(NodeFlags::DOM_LOOSE_NAME)
        || !node.flags.contains(NodeFlags::NS_RESOLVED)
    {
        return None;
    }
    for at in doc.attributes(el) {
        if xmlns_prefix(doc.qname(at)) == Some(&b""[..]) {
            return (doc.node(at).value.len != 0).then_some(at);
        }
    }
    None
}

/// What `prefix` ("" = default) is bound to at or above `node`, by the
/// declarations the mutators resolve against; empty when unbound. For a caller
/// deciding whether a declaration it is about to add would repeat one in scope.
pub fn namespace_in_scope<'d>(doc: &'d Document, node: NodeId, prefix: &[u8]) -> &'d [u8] {
    doc.span(resolve_in_scope(doc, Some(node), prefix))
}

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
        if doc.type_(id) == Some(ArenaKind::Element) {
            let ignored = ignored_default_decl(doc, id);
            for at in doc.attributes(id) {
                if let Some(p) = xmlns_prefix(doc.qname(at)) {
                    if p == prefix && Some(at) != ignored {
                        return doc.try_node(at).map_or(Span::EMPTY, |n| n.value);
                    }
                }
            }
        }
        e = doc.parent(id);
    }
    Span::EMPTY
}
