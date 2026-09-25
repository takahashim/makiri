//! The `[@name]` and `[@name='lit']` predicate shapes, recognised and matched.
//!
//! Its own module because TWO callers depend on the per-node test being the same
//! one: the predicate filter, and the at_xpath first-match walk that claims to
//! return exactly what the full evaluation would. Sharing the code is what makes
//! that claim true by construction rather than by a hand-kept copy.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;
use super::nodetest::unprefixed_attr_matches;

/// The two most common predicate shapes. The generic evaluator services them by
/// building a throwaway node-set per context node plus a string-cache insert;
/// recognising the shape filters with a direct attribute lookup instead. Both
/// are boolean - no position dependence - so this is a pure per-node filter with
/// the same result as the generic path.
pub struct AttrPred<'a> {
    pub name: &'a [u8],
    pub value: Option<&'a [u8]>,
}

/// Shape A: a relative path that is one unprefixed attribute name test with no
/// predicates - `@name`.
pub fn match_attr_step(e: &Expr) -> Option<&[u8]> {
    let ExprKind::Path(p) = &e.kind else {
        return None;
    };
    let [s] = p.steps.as_slice() else {
        return None;
    };
    if p.absolute
        || s.axis != Axis::Attribute
        || !s.predicates.is_empty()
        || s.test.kind != TestKind::Name
        || s.test.prefix.is_some()
    {
        return None;
    }
    s.test.local.as_deref()
}

/// `[@name]`, or `[@name='lit']` in either operand order.
pub fn match_attr_pred(p: &Expr) -> Option<AttrPred<'_>> {
    if let Some(name) = match_attr_step(p) {
        return Some(AttrPred { name, value: None });
    }
    let ExprKind::BinOp {
        op: Op::Eq,
        lhs,
        rhs,
    } = &p.kind
    else {
        return None;
    };
    let (lit, attr) = match string_literal(lhs) {
        Some(t) => (t, rhs),
        None => (string_literal(rhs)?, lhs),
    };
    let name = match_attr_step(attr)?;
    Some(AttrPred {
        name,
        value: Some(lit),
    })
}

/// The text of a string-literal node, or None for anything else.
fn string_literal(e: &Expr) -> Option<&[u8]> {
    match &e.kind {
        ExprKind::LiteralStr(t) => Some(t),
        _ => None,
    }
}

/// THE single per-node test for a recognised attribute predicate, shared by the
/// predicate filter and the at_xpath first-match path so the two stay identical
/// by construction rather than by a hand-kept copy.
///
/// It is also the attribute axis's own test: an attribute counts when
/// [`unprefixed_attr_matches`] - the function the axis's name test calls - says
/// so. And like `@name = 'v'` over that axis, the value compare is "some
/// matching attribute has it": in XML's lax mode `a` and `p:a` both match `@a`.
///
/// This scans rather than using the host's attribute lookup, whose rules are
/// not the name test's: Lexbor's folds case on every element, SVG included,
/// and sees the namespace declarations the axis hides.
pub fn attr_pred_matches<'d, D: Dom<'d>>(doc: D, ap: &AttrPred, n: D::Node, lax: bool) -> bool {
    /* Only an element has attributes, so a node of any other kind finds none. */
    doc.attributes(n).any(|x| {
        unprefixed_attr_matches(doc, Some(n), x, ap.name, lax)
            && ap.value.is_none_or(|want| doc.attr_value(x) == want)
    })
}
