//! Moving between elements: the parent, the element siblings the combinators
//! and `of S` step through, and the wider sibling set Lexbor's plain
//! `:nth-child` family counts.

use crate::lexbor::adapter::html::{HtmlNode, NodeType};

/// The next/previous sibling that is an ELEMENT - for combinator dispatch
/// (`+`, `~`, and the ancestor/parent climb) and `:nth-of-type`-family
/// checks, where the CSS spec (and Lexbor's own structural matching) only
/// ever considers actual elements. NOT for the plain `:nth-child`/
/// `:first-child`/`:last-child`/`:only-child` family - see
/// `next_position_sibling`/`prev_position_sibling` below for why those need
/// a different filter.
pub(super) fn next_sibling_element(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.next();
    while let Some(n) = cur {
        if n.element().is_some() {
            return Some(n);
        }
        cur = n.next();
    }
    None
}

pub(super) fn prev_sibling_element(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.prev();
    while let Some(n) = cur {
        if n.element().is_some() {
            return Some(n);
        }
        cur = n.prev();
    }
    None
}

pub(super) fn parent_element(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    node.parent().filter(|p| p.element().is_some())
}

/// Does `n` count as "a sibling" for the plain (no `of_type`, no `of S`)
/// `:nth-child`/`:nth-last-child`/`:first-child`/`:last-child`/`:only-child`
/// family? Lexbor's own rule (the sibling loop in
/// `lxb_selectors_pseudo_class_function`'s `NTH_CHILD` case, and
/// `lxb_selectors_pseudo_class_first_child`/`last_child` the same way):
/// everything except Text and Comment - NOT "is an element". A
/// `<!doctype html>` (a `DocumentType` node, `<html>`'s own preceding
/// "sibling" under the Document) or an HTML processing-instruction sibling
/// counts here even though neither is an element - found by
/// `agrees_with_the_old_engine_on_randomly_generated_selectors` generating
/// `*:nth-child(2n+1)` and disagreeing with the old engine on `<html>` itself:
/// this port's `prev_sibling_element`/`next_sibling_element` (element-only,
/// correct for combinators and `of_type`/`of S`, WRONG here) skipped the
/// doctype and undercounted its position by one.
fn counts_toward_child_position(n: HtmlNode<'_>) -> bool {
    !matches!(n.node_type(), NodeType::Text | NodeType::Comment)
}

pub(super) fn next_position_sibling(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.next();
    while let Some(n) = cur {
        if counts_toward_child_position(n) {
            return Some(n);
        }
        cur = n.next();
    }
    None
}

pub(super) fn prev_position_sibling(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.prev();
    while let Some(n) = cur {
        if counts_toward_child_position(n) {
            return Some(n);
        }
        cur = n.prev();
    }
    None
}

pub(super) fn name_matches_type(a: HtmlNode<'_>, b: HtmlNode<'_>) -> bool {
    // `lxb_selectors_pseudo_class_first_of_type` etc.: namespace-aware, unlike
    // the type selector itself (`lxb_selectors_match_element`, name-only).
    match (a.element(), b.element()) {
        (Some(ea), Some(eb)) => {
            a.ns_id() == b.ns_id() && ea.dom_local_name() == eb.dom_local_name()
        }
        _ => false,
    }
}

/// The next sibling ELEMENT in `:nth-*(of S)`'s counting direction - toward the
/// end for `:nth-last-child`, toward the start otherwise
/// (`lxb_selectors_pseudo_class_function`).
pub(super) fn nth_of_sibling(node: HtmlNode<'_>, from_end: bool) -> Option<HtmlNode<'_>> {
    if from_end {
        next_sibling_element(node)
    } else {
        prev_sibling_element(node)
    }
}
