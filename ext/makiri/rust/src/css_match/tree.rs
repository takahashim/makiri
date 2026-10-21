//! Moving between elements: the parent, the element siblings the combinators
//! and `of S` step through, and the wider sibling set Lexbor's plain
//! `:nth-child` family counts.

use crate::compat::{self, Fix};
use crate::node_type::NodeType;

use super::dom::{CssElement, CssNode};

/// The next/previous sibling that is an ELEMENT - for combinator dispatch
/// (`+`, `~`, and the ancestor/parent climb) and `:nth-of-type`-family
/// checks, where the CSS spec (and Lexbor's own structural matching) only
/// ever considers actual elements. NOT for the plain `:nth-child`/
/// `:first-child`/`:last-child`/`:only-child` family - see
/// `next_position_sibling`/`prev_position_sibling` below for why those need
/// a different filter.
pub(super) fn next_sibling_element<'d, N: CssNode<'d>>(node: N) -> Option<N> {
    let mut cur = node.next();
    while let Some(n) = cur {
        if n.element().is_some() {
            return Some(n);
        }
        cur = n.next();
    }
    None
}

pub(super) fn prev_sibling_element<'d, N: CssNode<'d>>(node: N) -> Option<N> {
    let mut cur = node.prev();
    while let Some(n) = cur {
        if n.element().is_some() {
            return Some(n);
        }
        cur = n.prev();
    }
    None
}

/// `node`'s children, in order.
pub(super) fn children<'d, N: CssNode<'d>>(node: N) -> impl Iterator<Item = N> {
    core::iter::successors(node.first_child(), |n| n.next())
}

pub(super) fn parent_element<'d, N: CssNode<'d>>(node: N) -> Option<N> {
    node.parent().filter(|p| p.element().is_some())
}

/// Does `n` count as "a sibling" for the plain (no `of_type`, no `of S`)
/// `:nth-child`/`:nth-last-child`/`:first-child`/`:last-child`/`:only-child`
/// family? Selectors 4 counts element siblings only (M8 in
/// docs/parser_differences.ja.md). Lexbor's rule (the sibling loop in
/// `lxb_selectors_pseudo_class_function`'s `NTH_CHILD` case, and
/// `lxb_selectors_pseudo_class_first_child`/`last_child` the same way) is
/// everything except Text and Comment, so a `<!doctype html>` before
/// `<html>` or a processing-instruction sibling took a position; the
/// harnesses that hold the matcher to Lexbor's engine run that rule.
fn counts_toward_child_position<'d, N: CssNode<'d>>(n: N) -> bool {
    if compat::fixed(Fix::M8) {
        n.element().is_some()
    } else {
        !matches!(n.node_type(), NodeType::Text | NodeType::Comment)
    }
}

pub(super) fn next_position_sibling<'d, N: CssNode<'d>>(node: N) -> Option<N> {
    let mut cur = node.next();
    while let Some(n) = cur {
        if counts_toward_child_position(n) {
            return Some(n);
        }
        cur = n.next();
    }
    None
}

pub(super) fn prev_position_sibling<'d, N: CssNode<'d>>(node: N) -> Option<N> {
    let mut cur = node.prev();
    while let Some(n) = cur {
        if counts_toward_child_position(n) {
            return Some(n);
        }
        cur = n.prev();
    }
    None
}

pub(super) fn name_matches_type<'d, N: CssNode<'d>>(a: N, b: N) -> bool {
    // `lxb_selectors_pseudo_class_first_of_type` etc.: namespace-aware, unlike
    // the type selector itself (`lxb_selectors_match_element`, name-only).
    match (a.element(), b.element()) {
        (Some(ea), Some(eb)) => a.same_namespace(b) && ea.dom_local_name() == eb.dom_local_name(),
        _ => false,
    }
}

/// The next sibling ELEMENT in `:nth-*(of S)`'s counting direction - toward the
/// end for `:nth-last-child`, toward the start otherwise
/// (`lxb_selectors_pseudo_class_function`).
pub(super) fn nth_of_sibling<'d, N: CssNode<'d>>(node: N, from_end: bool) -> Option<N> {
    if from_end {
        next_sibling_element(node)
    } else {
        prev_sibling_element(node)
    }
}
