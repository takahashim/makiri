//! The thirteen axes: how each one enumerates nodes from a context node, and
//! the three questions the step driver asks about an axis before walking it.
//!
//! No knowledge of node tests, predicates or values - an axis is just an order
//! over the tree, which is why it comes apart from the evaluator cleanly.

use super::abi::*;
use super::dom::*;

/// Pre-order DFS over `context`'s PROPER descendants, calling `visit` on each;
/// stops as soon as `visit` returns true. The shared body of the descendant and
/// descendant-or-self axes - the latter only visits `context` first.
///
/// # Safety
/// `context` must be a live handle, and the tree must not be mutated during the
/// walk - it navigates by following links it reads as it goes.
pub unsafe fn walk_descendants<D: Dom, F: FnMut(D::Node) -> bool>(
    doc: D::Doc,
    context: D::Node,
    visit: &mut F,
) -> bool {
    let mut n = D::first_child(doc, context);
    while !D::is_null(n) && n != context {
        if visit(n) {
            return true;
        }
        if !D::is_null(D::first_child(doc, n)) {
            n = D::first_child(doc, n);
        } else {
            while n != context && D::is_null(D::next(doc, n)) {
                n = D::parent(doc, n);
            }
            if n == context {
                break;
            }
            n = D::next(doc, n);
        }
    }
    false
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
///
/// # Safety
/// `context` must be a live handle.
pub unsafe fn axis_base<D: Dom>(doc: D::Doc, context: D::Node) -> D::Node {
    if D::node_type(doc, context) == NTYPE_ATTRIBUTE {
        let owner = D::parent(doc, context);
        if !D::is_null(owner) {
            return owner;
        }
    }
    context
}

///
/// # Safety
/// Same as `walk_descendants`: a live context, and no mutation while it runs.
pub unsafe fn walk_axis<D: Dom, F: FnMut(D::Node) -> bool>(
    doc: D::Doc,
    axis: Axis,
    context: D::Node,
    visit: &mut F,
) -> bool {
    match axis {
        Axis::SelfAxis => visit(context),
        Axis::Parent => {
            let p = D::parent(doc, context);
            !D::is_null(p) && visit(p)
        }
        Axis::Child => {
            let mut c = D::first_child(doc, context);
            while !D::is_null(c) {
                if visit(c) {
                    return true;
                }
                c = D::next(doc, c);
            }
            false
        }
        Axis::Attribute => {
            if D::node_type(doc, context) != NTYPE_ELEMENT {
                return false;
            }
            let mut a = D::first_attr(doc, context);
            while !D::is_null(a) {
                if visit(a) {
                    return true;
                }
                a = D::attr_next(doc, a);
            }
            false
        }
        Axis::DescendantOrSelf => visit(context) || walk_descendants::<D, F>(doc, context, visit),
        Axis::Descendant => walk_descendants::<D, F>(doc, context, visit),
        Axis::Ancestor => {
            let mut p = D::parent(doc, context);
            while !D::is_null(p) {
                if visit(p) {
                    return true;
                }
                p = D::parent(doc, p);
            }
            false
        }
        Axis::AncestorOrSelf => {
            let mut p = context;
            while !D::is_null(p) {
                if visit(p) {
                    return true;
                }
                p = D::parent(doc, p);
            }
            false
        }
        /* §2.2: both sibling axes are empty for an attribute context node - an
         * attribute is not a sibling of anything. */
        Axis::FollowingSibling => {
            if D::node_type(doc, context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = D::next(doc, context);
            while !D::is_null(s) {
                if visit(s) {
                    return true;
                }
                s = D::next(doc, s);
            }
            false
        }
        Axis::PrecedingSibling => {
            if D::node_type(doc, context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = D::prev(doc, context);
            while !D::is_null(s) {
                if visit(s) {
                    return true;
                }
                s = D::prev(doc, s);
            }
            false
        }
        Axis::Following => {
            /* Start at the next node in document order after the base's subtree. */
            let mut cur = axis_base::<D>(doc, context);
            while !D::is_null(cur) && D::is_null(D::next(doc, cur)) {
                cur = D::parent(doc, cur);
            }
            if D::is_null(cur) {
                return false;
            }
            cur = D::next(doc, cur);
            while !D::is_null(cur) {
                if visit(cur) {
                    return true;
                }
                if !D::is_null(D::first_child(doc, cur)) {
                    cur = D::first_child(doc, cur);
                } else {
                    while !D::is_null(cur) && D::is_null(D::next(doc, cur)) {
                        cur = D::parent(doc, cur);
                    }
                    if !D::is_null(cur) {
                        cur = D::next(doc, cur);
                    }
                }
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
            while !D::is_null(cur) {
                if !D::is_null(D::prev(doc, cur)) {
                    cur = D::prev(doc, cur);
                    while !D::is_null(D::last_child(doc, cur)) {
                        cur = D::last_child(doc, cur);
                    }
                    if visit(cur) {
                        return true;
                    }
                } else {
                    cur = D::parent(doc, cur);
                    if D::is_null(cur) {
                        return false;
                    }
                    let mut is_ancestor = false;
                    let mut p = D::parent(doc, context);
                    while !D::is_null(p) {
                        if p == cur {
                            is_ancestor = true;
                            break;
                        }
                        p = D::parent(doc, p);
                    }
                    if !is_ancestor && visit(cur) {
                        return true;
                    }
                }
            }
            false
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
