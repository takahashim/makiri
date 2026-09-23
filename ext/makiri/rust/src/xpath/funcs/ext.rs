//! The functions beyond XPath 1.0's own library: Nokogiri's two builtins, in
//! its builtin namespace, and the CSS lowering's internal of-type hooks, whose
//! names no expression can spell. `lookup` in the parent is still the one
//! place that knows which names exist.

#![forbid(unsafe_code)]

use super::*;

/* ---------- the Nokogiri builtins ---------- */

/// css-class(haystack, needle): true iff `needle` is a whitespace-separated
/// token of `haystack`. Kept behaviour-identical to libxml2's builtin_css_class,
/// including the NULL ordering - a NULL haystack is a non-match even for an
/// empty needle.
fn ws_token_match(hay: Option<&[u8]>, val: Option<&[u8]>) -> bool {
    let (hay, val) = match (hay, val) {
        (Some(h), Some(v)) => (h, v),
        _ => return false,
    };
    if val.is_empty() {
        return true; /* libxml2 returns non-NULL for an empty val */
    }
    hay.split(|&b| crate::xpath::lex::is_ws(b))
        .any(|t| t == val)
}

pub(super) fn fn_css_class<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    arity(args.len(), 2, 2, err.clone(), "nokogiri-builtin:css-class")?;
    two::<D, _>(ev, args, |hay, needle| {
        boolean(ws_token_match(Some(hay), Some(needle)))
    })
}

/// local-name-is(name): true iff the context node's qualified name (for HTML the
/// lowercase local name) equals the argument.
pub(super) fn fn_local_name_is<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    arity(
        args.len(),
        1,
        1,
        err.clone(),
        "nokogiri-builtin:local-name-is",
    )?;
    let want = to_text::<D>(&args[0], ev)?;
    boolean(
        focus
            .node
            .is_some_and(|n| doc.qualified_name(n) == want.as_slice()),
    )
}

/* ---------- the CSS-lowered of-type hooks (XML only) ---------- */

/// Two elements are the same "type" iff they share an expanded name: local name
/// plus namespace URI.
fn same_type<'e, 'd, D: Dom<'d>>(a: D::Node, b: D::Node, doc: D) -> bool {
    doc.local_name(a) == doc.local_name(b) && doc.ns_uri(a) == doc.ns_uri(b)
}

/// The 1-based position of `node` among its same-type element siblings: forward
/// counts the preceding siblings, otherwise the following ones (from the end).
///
/// A tick per sibling passed: CSS `:nth-of-type` over XML runs this for every
/// candidate, so a flat list of n siblings is n^2 steps - 40,000 took 15 s,
/// uncharged.
fn of_type_pos<'e, 'd, D: Dom<'d>>(
    node: Option<D::Node>,
    forward: bool,
    doc: D,
    budget: &Budget,
) -> Result<f64, Reported> {
    let Some(node) = node else {
        return Ok(0.0);
    };
    if doc.node_type(node) != NTYPE_ELEMENT {
        return Ok(0.0);
    }
    let step = |n: D::Node| {
        if forward {
            doc.prev(n)
        } else {
            doc.next(n)
        }
    };
    let mut pos = 1i64;
    let mut s = step(node);
    while let Some(n) = s {
        budget.charge_op()?;
        if doc.node_type(n) == NTYPE_ELEMENT && same_type::<D>(node, n, doc) {
            pos += 1;
        }
        s = step(n);
    }
    Ok(pos as f64)
}

pub(super) fn fn_of_type_pos<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    number(of_type_pos::<D>(focus.node, true, doc, &ev.budget)?)
}

pub(super) fn fn_of_type_pos_last<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    number(of_type_pos::<D>(focus.node, false, doc, &ev.budget)?)
}
