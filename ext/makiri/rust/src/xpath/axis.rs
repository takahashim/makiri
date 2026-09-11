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
pub unsafe fn walk_descendants<D: Dom, F: FnMut(D::Node) -> bool>(
    context: D::Node,
    visit: &mut F,
) -> bool {
    let mut n = D::first_child(context);
    while !D::is_null(n) && n != context {
        if visit(n) {
            return true;
        }
        if !D::is_null(D::first_child(n)) {
            n = D::first_child(n);
        } else {
            while n != context && D::is_null(D::next(n)) {
                n = D::parent(n);
            }
            if n == context {
                break;
            }
            n = D::next(n);
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
pub unsafe fn axis_base<D: Dom>(context: D::Node) -> D::Node {
    if D::node_type(context) == NTYPE_ATTRIBUTE {
        let owner = D::parent(context);
        if !D::is_null(owner) {
            return owner;
        }
    }
    context
}

pub unsafe fn walk_axis<D: Dom, F: FnMut(D::Node) -> bool>(
    axis: u32,
    context: D::Node,
    visit: &mut F,
) -> bool {
    match axis {
        AXIS_SELF => visit(context),
        AXIS_PARENT => {
            let p = D::parent(context);
            !D::is_null(p) && visit(p)
        }
        AXIS_CHILD => {
            let mut c = D::first_child(context);
            while !D::is_null(c) {
                if visit(c) {
                    return true;
                }
                c = D::next(c);
            }
            false
        }
        AXIS_ATTRIBUTE => {
            if D::node_type(context) != NTYPE_ELEMENT {
                return false;
            }
            let mut a = D::first_attr(context);
            while !D::is_null(a) {
                if visit(a) {
                    return true;
                }
                a = D::attr_next(a);
            }
            false
        }
        AXIS_DESCENDANT_OR_SELF => visit(context) || walk_descendants::<D, F>(context, visit),
        AXIS_DESCENDANT => walk_descendants::<D, F>(context, visit),
        AXIS_ANCESTOR => {
            let mut p = D::parent(context);
            while !D::is_null(p) {
                if visit(p) {
                    return true;
                }
                p = D::parent(p);
            }
            false
        }
        AXIS_ANCESTOR_OR_SELF => {
            let mut p = context;
            while !D::is_null(p) {
                if visit(p) {
                    return true;
                }
                p = D::parent(p);
            }
            false
        }
        /* §2.2: both sibling axes are empty for an attribute context node - an
         * attribute is not a sibling of anything. */
        AXIS_FOLLOWING_SIBLING => {
            if D::node_type(context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = D::next(context);
            while !D::is_null(s) {
                if visit(s) {
                    return true;
                }
                s = D::next(s);
            }
            false
        }
        AXIS_PRECEDING_SIBLING => {
            if D::node_type(context) == NTYPE_ATTRIBUTE {
                return false;
            }
            let mut s = D::prev(context);
            while !D::is_null(s) {
                if visit(s) {
                    return true;
                }
                s = D::prev(s);
            }
            false
        }
        AXIS_FOLLOWING => {
            /* Start at the next node in document order after the base's subtree. */
            let mut cur = axis_base::<D>(context);
            while !D::is_null(cur) && D::is_null(D::next(cur)) {
                cur = D::parent(cur);
            }
            if D::is_null(cur) {
                return false;
            }
            cur = D::next(cur);
            while !D::is_null(cur) {
                if visit(cur) {
                    return true;
                }
                if !D::is_null(D::first_child(cur)) {
                    cur = D::first_child(cur);
                } else {
                    while !D::is_null(cur) && D::is_null(D::next(cur)) {
                        cur = D::parent(cur);
                    }
                    if !D::is_null(cur) {
                        cur = D::next(cur);
                    }
                }
            }
            false
        }
        AXIS_PRECEDING => {
            /* Backward in document order, skipping the context's ancestors, so
             * the closest preceding node comes first.
             *
             * Climbing to a parent reaches an ancestor only when we are climbing
             * the chain from the context itself, not when climbing back out of a
             * preceding sibling's subtree - hence the explicit test. It stays
             * anchored on `context`, not the base: for an attribute context node
             * the owner element IS an ancestor (§2.2), so starting the walk there
             * must not emit it. */
            let mut cur = axis_base::<D>(context);
            while !D::is_null(cur) {
                if !D::is_null(D::prev(cur)) {
                    cur = D::prev(cur);
                    while !D::is_null(D::last_child(cur)) {
                        cur = D::last_child(cur);
                    }
                    if visit(cur) {
                        return true;
                    }
                } else {
                    cur = D::parent(cur);
                    if D::is_null(cur) {
                        return false;
                    }
                    let mut is_ancestor = false;
                    let mut p = D::parent(context);
                    while !D::is_null(p) {
                        if p == cur {
                            is_ancestor = true;
                            break;
                        }
                        p = D::parent(p);
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
pub fn axis_can_alias(a: u32) -> bool {
    !matches!(a, AXIS_CHILD | AXIS_ATTRIBUTE | AXIS_SELF)
}

pub fn axis_is_implemented(a: u32) -> bool {
    a != AXIS_NAMESPACE && a <= AXIS_ANCESTOR_OR_SELF
}

pub fn axis_name(a: u32) -> &'static str {
    match a {
        AXIS_ANCESTOR => "ancestor",
        AXIS_ANCESTOR_OR_SELF => "ancestor-or-self",
        AXIS_FOLLOWING => "following",
        AXIS_PRECEDING => "preceding",
        AXIS_FOLLOWING_SIBLING => "following-sibling",
        AXIS_PRECEDING_SIBLING => "preceding-sibling",
        AXIS_NAMESPACE => "namespace",
        _ => "axis",
    }
}

/// A reverse axis in the §2.4 sense: the walker emits in reverse-document order
/// and proximity position() counts outward from the context node. The step
/// driver applies predicates in that axis-natural order (so `[1]` is the
/// closest), then sorts the merged result into document order.
pub fn is_reverse_axis(a: u32) -> bool {
    matches!(
        a,
        AXIS_ANCESTOR | AXIS_ANCESTOR_OR_SELF | AXIS_PRECEDING | AXIS_PRECEDING_SIBLING
    )
}
