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
pub unsafe fn match_attr_step<'a>(n: *const Node) -> Option<&'a [u8]> {
    if n.is_null() || (*n).kind != NK_PATH || (*n).u.path.absolute != 0 || (*n).u.path.nsteps != 1 {
        return None;
    }
    let s = &*(*n).u.path.steps;
    if s.axis != AXIS_ATTRIBUTE
        || s.npredicates != 0
        || s.test.kind != NT_NAME
        || !s.test.prefix.ptr.is_null()
        || s.test.local.ptr.is_null()
    {
        return None;
    }
    Some(owned_bytes(s.test.local))
}

/// `[@name]`, or `[@name='lit']` in either operand order.
pub unsafe fn match_attr_pred<'a>(p: *const Node) -> Option<AttrPred<'a>> {
    if let Some(name) = match_attr_step(p) {
        return Some(AttrPred { name, value: None });
    }
    if p.is_null() || (*p).kind != NK_BINOP || (*p).u.binop.op != OP_EQ {
        return None;
    }
    let (lhs, rhs) = ((*p).u.binop.lhs, (*p).u.binop.rhs);
    let (lit, attr) = if !lhs.is_null() && (*lhs).kind == NK_LITERAL_STR {
        (lhs, rhs)
    } else if !rhs.is_null() && (*rhs).kind == NK_LITERAL_STR {
        (rhs, lhs)
    } else {
        return None;
    };
    let name = match_attr_step(attr)?;
    Some(AttrPred { name, value: Some(owned_bytes((*lit).u.literal)) })
}

/// The attribute whose QUALIFIED name is exactly `name`, case-sensitively.
///
/// This scans rather than using the host's attribute lookup, because Lexbor's is
/// HTML case-INsensitive - which would make `[@Id]` match `id`, diverging from
/// XPath 1.0, from Nokogiri::HTML5, and from Makiri's own attribute-axis name
/// test, which compares the qualified name byte for byte. The fast path handles
/// unprefixed names only, matching that comparison.
unsafe fn attr_by_qualified_name<D: Dom>(el: D::Node, name: &[u8]) -> D::Node {
    let mut a = D::first_attr(el);
    while !D::is_null(a) {
        if D::attr_qualified_name(a) == name {
            return a;
        }
        a = D::attr_next(a);
    }
    D::null()
}

/// THE single per-node test for a recognised attribute predicate, shared by the
/// predicate filter and the at_xpath first-match path so the two stay identical
/// by construction rather than by a hand-kept copy.
pub unsafe fn attr_pred_matches<D: Dom>(ap: &AttrPred, n: D::Node) -> bool {
    if D::node_type(n) != NTYPE_ELEMENT {
        return false;
    }
    let a = attr_by_qualified_name::<D>(n, ap.name);
    if D::is_null(a) {
        return false;
    }
    match ap.value {
        None => true,
        Some(want) => D::attr_value(a) == want,
    }
}
