//! The thirteen axes: how each one enumerates nodes from a context node, and
//! the three questions the step driver asks about an axis before walking it.
//!
//! No knowledge of node tests, predicates or values - an axis is just an order
//! over the tree, which is why it comes apart from the evaluator cleanly.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;
use core::ops::ControlFlow;

/// Pre-order DFS over `context`'s PROPER descendants, calling `visit` on each;
/// stops as soon as `visit` breaks, and hands the break back. The shared body of the descendant and
/// descendant-or-self axes - the latter only visits `context` first.
///
/// It navigates by the links it reads as it goes, and never leaves `context`'s
/// subtree.
pub fn walk_descendants<'d, D: Dom<'d>, B, F: FnMut(D::Node) -> ControlFlow<B>>(
    doc: D,
    context: D::Node,
    visit: &mut F,
) -> ControlFlow<B> {
    let mut cur = doc.first_child(context);
    while let Some(n) = cur {
        if n == context {
            break;
        }
        visit(n)?;
        if let Some(c) = doc.first_child(n) {
            cur = Some(c);
            continue;
        }
        cur = next_within(doc, n, context);
    }
    ControlFlow::Continue(())
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
/// stops as soon as `visit` breaks, and hands the break back - a budget overrun,
/// or a caller's "found it".
pub fn walk_axis<'d, D: Dom<'d>, B, F: FnMut(D::Node) -> ControlFlow<B>>(
    doc: D,
    axis: Axis,
    context: D::Node,
    visit: &mut F,
) -> ControlFlow<B> {
    match axis {
        Axis::SelfAxis => visit(context),
        Axis::Parent => match doc.parent(context) {
            Some(p) => visit(p),
            None => ControlFlow::Continue(()),
        },
        Axis::Child => {
            let mut c = doc.first_child(context);
            while let Some(n) = c {
                visit(n)?;
                c = doc.next(n);
            }
            ControlFlow::Continue(())
        }
        Axis::Attribute => {
            /* None for anything but an element: see `attrs`. */
            for x in doc.attributes(context) {
                visit(D::attr_node(x))?;
            }
            ControlFlow::Continue(())
        }
        Axis::DescendantOrSelf => {
            visit(context)?;
            walk_descendants::<D, B, F>(doc, context, visit)
        }
        Axis::Descendant => walk_descendants::<D, B, F>(doc, context, visit),
        Axis::Ancestor => {
            let mut p = doc.parent(context);
            while let Some(n) = p {
                visit(n)?;
                p = doc.parent(n);
            }
            ControlFlow::Continue(())
        }
        Axis::AncestorOrSelf => {
            let mut p = Some(context);
            while let Some(n) = p {
                visit(n)?;
                p = doc.parent(n);
            }
            ControlFlow::Continue(())
        }
        /* §2.2: both sibling axes are empty for an attribute context node - an
         * attribute is not a sibling of anything. */
        Axis::FollowingSibling => {
            if doc.node_type(context) == NTYPE_ATTRIBUTE {
                return ControlFlow::Continue(());
            }
            let mut s = doc.next(context);
            while let Some(n) = s {
                visit(n)?;
                s = doc.next(n);
            }
            ControlFlow::Continue(())
        }
        Axis::PrecedingSibling => {
            if doc.node_type(context) == NTYPE_ATTRIBUTE {
                return ControlFlow::Continue(());
            }
            let mut s = doc.prev(context);
            while let Some(n) = s {
                visit(n)?;
                s = doc.prev(n);
            }
            ControlFlow::Continue(())
        }
        Axis::Following => {
            /* Start at the next node in document order after the base's subtree. */
            let mut cur = next_after(doc, axis_base::<D>(doc, context));
            while let Some(n) = cur {
                visit(n)?;
                cur = match doc.first_child(n) {
                    Some(c) => Some(c),
                    None => next_after(doc, n),
                };
            }
            ControlFlow::Continue(())
        }
        Axis::Preceding => {
            /* Backward in document order, skipping the context's ancestors, so
             * the closest preceding node comes first.
             *
             * Climbing to a parent reaches an ancestor only when we are climbing
             * the chain from the base itself, not when climbing back out of a
             * preceding sibling's subtree - and the chain is met nearest first.
             * So the one ancestor still to be skipped is enough to recognise
             * each: comparing against the whole chain on every climb cost
             * O(depth) a climb and O(depth^2) a context node, none of it charged
             * to the budget - `//span/preceding::a` over 2000 nested spans ran
             * for nine seconds. (libxml2's xmlXPathNextPrecedingInternal keeps
             * the same single ancestor.)
             *
             * The chain starts above the BASE: for an attribute context node the
             * owner element is an ancestor too (§2.2), and the walk starts there,
             * so it is never climbed to and cannot be emitted. */
            let mut cur = axis_base::<D>(doc, context);
            let mut next_ancestor = doc.parent(cur);
            loop {
                if let Some(p) = doc.prev(cur) {
                    cur = p;
                    while let Some(l) = doc.last_child(cur) {
                        cur = l;
                    }
                    visit(cur)?;
                } else {
                    let Some(p) = doc.parent(cur) else {
                        return ControlFlow::Continue(());
                    };
                    cur = p;
                    if Some(cur) == next_ancestor {
                        next_ancestor = doc.parent(cur);
                    } else {
                        visit(cur)?;
                    }
                }
            }
        }
        /* Rejected by the step driver before it gets here. Named rather than
         * `_`, so a new axis is a compile error here, not a silent no-op. */
        Axis::Namespace => ControlFlow::Continue(()),
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
