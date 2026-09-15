//! The thirteen axes: how each one enumerates nodes from a context node, and
//! the three questions the step driver asks about an axis before walking it.
//!
//! No knowledge of node tests, predicates or values - an axis is just an order
//! over the tree, which is why it comes apart from the evaluator cleanly.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;

/// Pre-order DFS over `context`'s PROPER descendants, calling `visit` on each;
/// stops as soon as `visit` returns true. The shared body of the descendant and
/// descendant-or-self axes - the latter only visits `context` first.
///
/// It navigates by the links it reads as it goes, and never leaves `context`'s
/// subtree.
pub fn walk_descendants<'d, D: Dom<'d>, F: FnMut(D::Node) -> bool>(
    doc: D,
    context: D::Node,
    visit: &mut F,
) -> bool {
    let mut cur = doc.first_child(context);
    while let Some(n) = cur {
        if n == context {
            break;
        }
        if visit(n) {
            return true;
        }
        if let Some(c) = doc.first_child(n) {
            cur = Some(c);
            continue;
        }
        cur = next_within(doc, n, context);
    }
    false
}

/// The node after `n`'s subtree in document order, without leaving `root`'s
/// subtree: `n`'s next sibling, or the next sibling of its nearest ancestor
/// below `root` that has one.
fn next_within<'d, D: Dom<'d>>(doc: D, mut n: D::Node, root: D::Node) -> Option<D::Node> {
    loop {
        if n == root {
            return None;
        }
        if let Some(s) = doc.next(n) {
            return Some(s);
        }
        n = doc.parent(n)?;
    }
}

/// The node after `n`'s subtree in document order, anywhere in the tree.
fn next_after<'d, D: Dom<'d>>(doc: D, mut n: D::Node) -> Option<D::Node> {
    loop {
        if let Some(s) = doc.next(n) {
            return Some(s);
        }
        n = doc.parent(n)?;
    }
}

/// Where a document-order axis walk starts. For an attribute context node that
/// is its owner element.
///
/// §2.2 keeps attribute nodes out of the following / preceding axes, and §5.3
/// puts an element's attributes before its children, so a walk beginning at the
/// attribute itself would have to skip past the rest of the attribute list.
/// Starting at the owner gets there directly, and matches libxml2:
/// `following::node()` from an attribute yields what comes after the owner
/// element's subtree, not the element's own children.
pub fn axis_base<'d, D: Dom<'d>>(doc: D, context: D::Node) -> D::Node {
    if doc.node_type(context) == NTYPE_ATTRIBUTE {
        if let Some(owner) = doc.parent(context) {
            return owner;
        }
    }
    context
}

/// Call `visit` on each node of `axis` from `context`, in the axis's order;
/// stops as soon as `visit` returns true, and says whether it did.
pub fn walk_axis<'d, D: Dom<'d>, F: FnMut(D::Node) -> bool>(
    doc: D,
    axis: Axis,
    context: D::Node,
    visit: &mut F,
) -> bool {
    match axis {
        Axis::SelfAxis => visit(context),
        Axis::Parent => match doc.parent(context) {
            Some(p) => visit(p),
            None => false,
        },
        Axis::Child => {
            let mut c = doc.first_child(context);
            while let Some(n) = c {
                if visit(n) {
                    return true;
                }
                c = doc.next(n);
            }
            false
        }
        Axis::Attribute => {
            /* `first_attr` is None for anything but an element. */
            let mut a = doc.first_attr(context);
            while let Some(x) = a {
                if visit(D::attr_node(x)) {
                    return true;
                }
                a = doc.attr_next(x);
            }
            false
        }
        Axis::DescendantOrSelf => visit(context) || walk_descendants::<D, F>(doc, context, visit),
        Axis::Descendant => walk_descendants::<D, F>(doc, context, visit),
        Axis::Ancestor => {
            let mut p = doc.parent(context);
            while let Some(n) = p {
                if visit(n) {
                    return true;
                }
                p = doc.parent(n);
            }
            false
        }
        Axis::AncestorOrSelf => {
            let mut p = Some(context);
            while let Some(n) = p {
                if visit(n) {
                    return true;
                }
                p = doc.parent(n);
            }
            false
        }
        /* §2.2: both sibling axes are empty for an attribute context node - an
         * attribute is not a sibling of anything. */
        Axis::FollowingSibling => {
            if doc.node_type(context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = doc.next(context);
            while let Some(n) = s {
                if visit(n) {
                    return true;
                }
                s = doc.next(n);
            }
            false
        }
        Axis::PrecedingSibling => {
            if doc.node_type(context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = doc.prev(context);
            while let Some(n) = s {
                if visit(n) {
                    return true;
                }
                s = doc.prev(n);
            }
            false
        }
        Axis::Following => {
            /* Start at the next node in document order after the base's subtree. */
            let mut cur = next_after(doc, axis_base::<D>(doc, context));
            while let Some(n) = cur {
                if visit(n) {
                    return true;
                }
                cur = match doc.first_child(n) {
                    Some(c) => Some(c),
                    None => next_after(doc, n),
                };
            }
            false
        }
        Axis::Preceding => {
            /* Backward in document order, skipping the context's ancestors, so
             * the closest preceding node comes first.
             *
             * Climbing to a parent reaches an ancestor only when we are climbing
             * the chain from the context itself, not when climbing back out of a
             * preceding sibling's subtree - hence the explicit test. It stays
             * anchored on `context`, not the base: for an attribute context node
             * the owner element IS an ancestor (§2.2), so starting the walk there
             * must not emit it. */
            let mut cur = axis_base::<D>(doc, context);
            loop {
                if let Some(p) = doc.prev(cur) {
                    cur = p;
                    while let Some(l) = doc.last_child(cur) {
                        cur = l;
                    }
                    if visit(cur) {
                        return true;
                    }
                } else {
                    let Some(p) = doc.parent(cur) else {
                        return false;
                    };
                    cur = p;
                    let mut is_ancestor = false;
                    let mut a = doc.parent(context);
                    while let Some(x) = a {
                        if x == cur {
                            is_ancestor = true;
                            break;
                        }
                        a = doc.parent(x);
                    }
                    if !is_ancestor && visit(cur) {
                        return true;
                    }
                }
            }
        }
        /* The namespace axis is rejected by the step driver before it gets here. */
        _ => false,
    }
}

/// Can walking `axis` from distinct context nodes yield the same node twice?
///
/// child, attribute and self each anchor a result to one starting node, so
/// distinct contexts give distinct results. Everything else can overlap: two
/// contexts share a parent, or sit in an ancestor-descendant relation.
pub fn axis_can_alias(a: Axis) -> bool {
    !matches!(a, Axis::Child | Axis::Attribute | Axis::SelfAxis)
}

pub fn axis_is_implemented(a: Axis) -> bool {
    a != Axis::Namespace
}

pub fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::Ancestor => "ancestor",
        Axis::AncestorOrSelf => "ancestor-or-self",
        Axis::Following => "following",
        Axis::Preceding => "preceding",
        Axis::FollowingSibling => "following-sibling",
        Axis::PrecedingSibling => "preceding-sibling",
        Axis::Namespace => "namespace",
        _ => "axis",
    }
}

/// A reverse axis in the §2.4 sense: the walker emits in reverse-document order
/// and proximity position() counts outward from the context node. The step
/// driver applies predicates in that axis-natural order (so `[1]` is the
/// closest), then sorts the merged result into document order.
pub fn is_reverse_axis(a: Axis) -> bool {
    matches!(
        a,
        Axis::Ancestor | Axis::AncestorOrSelf | Axis::Preceding | Axis::PrecedingSibling
    )
}
