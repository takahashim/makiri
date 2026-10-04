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
//! element, and [`resolve_into`] is all-or-nothing - a pass that plans every
//! resolution over the unchanged tree, and, only if every prefix binds, a pass
//! that applies the plan.

#![forbid(unsafe_code)]

use crate::falloc::{OomResult, VecPush};
use crate::xml::ns_scope::{resolve_in_scope, Placement};
use crate::xml::qname::{name_ns, NameNs, NameRole, ReservedPrefix, Split};
use crate::xml::{ArenaKind, AttrNs, Document, MutError, NodeFlags, NodeId, Span};

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

    /// Record the outcome on attribute `attr`. Resolution only ever produces
    /// the derived or pending state; `Explicit` is the caller's to set.
    pub(super) fn write_attr(self, doc: &mut Document, attr: NodeId) {
        let n = doc.node_mut(attr);
        n.ns_uri = self.ns;
        n.attr_ns = if self.pending {
            AttrNs::Pending
        } else {
            AttrNs::Derived
        };
    }
}

/// Under what a resolution runs: whether the node is (or is about to be)
/// connected - an unbound prefix is then refused rather than deferred - and,
/// for a subtree about to be placed, where ([`Placement`]).
#[derive(Clone, Copy)]
pub(super) struct Resolution {
    pub(super) connected: bool,
    pub(super) placed: Option<Placement>,
}

/// Resolve `name` (split per `sp`) applied at `scope`, by the parser's rules
/// ([`name_ns`]) with the declarations at or above `scope`. An unbound prefix
/// is an error only when connected; deferred - and reported pending -
/// otherwise.
pub(super) fn resolve_ns(
    doc: &Document,
    scope: Option<NodeId>,
    name: &[u8],
    sp: &Split,
    is_attr: bool,
    how: Resolution,
) -> Result<Resolved, MutError> {
    let role = if is_attr {
        NameRole::Attribute
    } else {
        NameRole::Element
    };
    let lookup = |prefix: &[u8]| Some(resolve_in_scope(doc, scope, prefix, how.placed));
    match name_ns(doc, name, sp, role, lookup).map_err(|ReservedPrefix| MutError::BadName)? {
        NameNs::Uri(ns) => Ok(Resolved::decided(ns)),
        NameNs::Unbound if how.connected => Err(MutError::UnboundNs),
        NameNs::Unbound => Ok(Resolved {
            ns: NO_NS,
            pending: true,
        }),
    }
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
    let state = doc.attr_ns_state(attr);
    state != Some(AttrNs::Explicit) && (part == Part::Whole || state == Some(AttrNs::Pending))
}

/// Whether `e`'s own name is resolved for this `part`.
fn resolves_name(doc: &Document, e: NodeId, part: Part) -> bool {
    part == Part::Whole && !doc.is_loose_name(e)
}

/// The all-or-nothing plan the check pass produces and the apply pass writes:
/// the element names and attributes whose namespace CHANGES.
///
/// The commit is a pure application, so it can neither fail nor disagree with
/// the check - and every resolution is computed against the same pre-write
/// tree, which a write-then-resolve pass could not promise.
///
/// It holds only changes, so what it costs follows what the insertion does,
/// not the subtree's size: moving a decided subtree plans nothing. The
/// `NS_RESOLVED` stamp is not planned at all - see [`apply_ns_plan`].
#[derive(Default)]
struct NsPlan {
    /// Element name -> its new `ns_uri`, when that differs from the stored one.
    names: Vec<(NodeId, Ns)>,
    /// Attribute -> its new namespace and pending bit.
    attrs: Vec<(NodeId, Resolved)>,
}

/// The check half for element `e` - see [`Part`]: that every prefix binds, and
/// that its attributes' keys stay unique, the rule the parser holds a document
/// to (§3). Records what to write in `plan` instead of writing. Takes
/// `&Document`, so the pass that must write nothing cannot.
fn plan_node_ns(
    doc: &Document,
    e: NodeId,
    how: Resolution,
    part: Part,
    plan: &mut NsPlan,
) -> Result<(), MutError> {
    if resolves_name(doc, e, part) {
        let r = resolve_ns(doc, Some(e), doc.qname(e), &doc.split_of(e), false, how)?;
        if r.ns != doc.node(e).ns_uri {
            plan.names.falloc_push((e, r.ns)).or_oom::<MutError>()?;
        }
    }
    /* Every attribute's key as it will stand - a re-resolved one's new
     * namespace, anyone else's stored one - leaving out those still pending,
     * which have no namespace to compare yet. */
    let mut keys: Vec<(Span, NodeId)> = Vec::new();
    for attr in doc.attributes(e) {
        let (key, write) = if rederives(doc, attr, part) {
            let r = resolve_ns(
                doc,
                Some(e),
                doc.qname(attr),
                &doc.split_of(attr),
                true,
                how,
            )?;
            ((!r.pending).then_some(r.ns), Some(r))
        } else {
            (Some(doc.node(attr).ns_uri), None)
        };
        if let Some(ns) = key {
            keys.falloc_push((ns, attr)).or_oom::<MutError>()?;
        }
        if let Some(r) = write {
            plan.attrs.falloc_push((attr, r)).or_oom::<MutError>()?;
        }
    }
    if crate::xml::attr_key::keys_repeat(doc, &mut keys) {
        return Err(MutError::DuplicateAttr);
    }
    Ok(())
}

/// Write what the plan decided, then - when `root` is connected - stamp every
/// element of its subtree `NS_RESOLVED`. Infallible: nothing here resolves a
/// name or allocates.
///
/// The stamp needs no plan entry: once a connected insertion has succeeded,
/// every element under `root` is decided - the planned ones just were, and the
/// ones the plan skipped were already. A detached subtree is not stamped,
/// because resolution there is deferred (an unbound prefix is not an error
/// yet), so its nodes must stay open to resolving again when it joins the
/// document.
fn apply_ns_plan(doc: &mut Document, root: NodeId, connected: bool, plan: NsPlan) {
    for (e, ns) in plan.names {
        doc.node_mut(e).ns_uri = ns;
    }
    for (attr, r) in plan.attrs {
        r.write_attr(doc, attr);
    }
    if connected {
        let mut cur = Some(root);
        while let Some(c) = cur {
            if doc.type_(c) == Some(ArenaKind::Element) {
                doc.node_mut(c).flags.insert(NodeFlags::NS_RESOLVED);
            }
            cur = doc.preorder_next(root, c);
        }
    }
}

/// Whether any attribute of `e` still has a pending namespace.
fn has_pending_attr(doc: &Document, e: NodeId) -> bool {
    for attr in doc.attributes(e) {
        if doc.attr_ns_state(attr) == Some(AttrNs::Pending) {
            return true;
        }
    }
    false
}

/// Resolve `node`'s subtree as if it were a child of `context`, without
/// linking it, all-or-nothing: plan every element over the tree as it stands,
/// reading `node`'s ancestors as `context`'s ([`Placement`]) - and only when
/// every prefix binds, apply the plan. For a DOCUMENT_FRAGMENT that is every
/// child about to be spliced, planned as one.
///
/// Only the apply writes: the planning reads the tree, as it is - an earlier
/// version linked `node` under `context` for the walk and restored the link
/// after, which a panic between the two would have left in place.
pub(super) fn resolve_into(
    doc: &mut Document,
    node: NodeId,
    context: NodeId,
) -> Result<(), MutError> {
    let how = Resolution {
        connected: doc.is_connected(context),
        placed: Some(Placement {
            root: node,
            context,
        }),
    };
    let plan = plan_subtree(doc, node, how)?;
    apply_ns_plan(doc, node, how.connected, plan);
    Ok(())
}

/// The plan for every element in `root`'s subtree, over the unchanged tree.
fn plan_subtree(doc: &Document, root: NodeId, how: Resolution) -> Result<NsPlan, MutError> {
    let mut plan = NsPlan::default();
    let mut cur = Some(root);
    while let Some(c) = cur {
        if doc.type_(c) == Some(ArenaKind::Element) {
            /* A decided element keeps its own namespace; its attributes set
             * while it was detached may still be pending. */
            let decided = doc.element_ns_decided(c);
            if !decided || has_pending_attr(doc, c) {
                let part = if decided {
                    Part::PendingAttrs
                } else {
                    Part::Whole
                };
                plan_node_ns(doc, c, how, part, &mut plan)?;
            }
        }
        cur = doc.preorder_next(root, c);
    }
    Ok(plan)
}
