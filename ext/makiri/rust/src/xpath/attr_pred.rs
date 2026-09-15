//! The `[@name]` and `[@name='lit']` predicate shapes, recognised and matched.
//!
//! Its own module because TWO callers depend on the per-node test being the same
//! one: the predicate filter, and the at_xpath first-match walk that claims to
//! return exactly what the full evaluation would. Sharing the code is what makes
//! that claim true by construction rather than by a hand-kept copy.

use super::abi::*;
use super::dom::*;
use super::value::owned_bytes;

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
///
/// # Safety
/// `n` must be NULL or a live AST node, and the returned name borrows that
/// node's owned text.
pub unsafe fn match_attr_step<'a>(n: *const Node) -> Option<&'a [u8]> {
    if n.is_null() {
        return None;
    }
    let NodeRef::Path(p) = Node::view(n) else {
        return None;
    };
    if p.absolute != 0 || p.nsteps != 1 {
        return None;
    }
    let s = &*p.steps;
    if s.axis != Axis::Attribute
        || s.npredicates != 0
        || s.test.kind != TestKind::Name
        || s.test.prefix.is_present()
        || s.test.local.is_absent()
    {
        return None;
    }
    Some(owned_bytes(s.test.local))
}

/// `[@name]`, or `[@name='lit']` in either operand order.
///
/// # Safety
/// Same as `match_attr_step`.
pub unsafe fn match_attr_pred<'a>(p: *const Node) -> Option<AttrPred<'a>> {
    if let Some(name) = match_attr_step(p) {
        return Some(AttrPred { name, value: None });
    }
    if p.is_null() {
        return None;
    }
    let NodeRef::BinOp(b) = Node::view(p) else {
        return None;
    };
    if b.op != Op::Eq {
        return None;
    }
    let (lit, attr) = match string_literal(b.lhs) {
        Some(t) => (t, b.rhs),
        None => (string_literal(b.rhs)?, b.lhs),
    };
    let name = match_attr_step(attr)?;
    Some(AttrPred {
        name,
        value: Some(owned_bytes(lit)),
    })
}

/// The text of a string-literal node, or None for anything else.
///
/// # Safety
/// `n` must be NULL or a live AST node.
unsafe fn string_literal(n: *const Node) -> Option<TextSlot> {
    if n.is_null() {
        return None;
    }
    match Node::view(n) {
        NodeRef::LiteralStr(t) => Some(t),
        _ => None,
    }
}

/// The attribute whose QUALIFIED name is exactly `name`, case-sensitively.
///
/// This scans rather than using the host's attribute lookup, because Lexbor's is
/// HTML case-INsensitive - which would make `[@Id]` match `id`, diverging from
/// XPath 1.0, from Nokogiri::HTML5, and from Makiri's own attribute-axis name
/// test, which compares the qualified name byte for byte. The fast path handles
/// unprefixed names only, matching that comparison.
unsafe fn attr_by_qualified_name<D: Dom>(doc: D::Doc, el: D::Node, name: &[u8]) -> D::Node {
    let mut a = D::first_attr(doc, el);
    while !D::is_null(a) {
        if D::attr_qualified_name(doc, a) == name {
            return a;
        }
        a = D::attr_next(doc, a);
    }
    D::null()
}

/// THE single per-node test for a recognised attribute predicate, shared by the
/// predicate filter and the at_xpath first-match path so the two stay identical
/// by construction rather than by a hand-kept copy.
///
/// # Safety
/// `n` must be a live handle of the document being evaluated, and `ap` must
/// still borrow the AST it came from.
pub unsafe fn attr_pred_matches<D: Dom>(doc: D::Doc, ap: &AttrPred, n: D::Node) -> bool {
    if D::node_type(doc, n) != NTYPE_ELEMENT {
        return false;
    }
    let a = attr_by_qualified_name::<D>(doc, n, ap.name);
    if D::is_null(a) {
        return false;
    }
    match ap.value {
        None => true,
        Some(want) => D::attr_value(doc, a) == want,
    }
}
