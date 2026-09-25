//! The two element-index fast paths: `//tag` and `//tag[N]`.
//!
//! Both answer a document-rooted descendant name test from the document's
//! element index instead of walking the tree, and both are pure optimisation -
//! each returns Ok(None) whenever the shape or the index cannot serve it, and
//! the caller walks. Keeping them out of the step driver keeps that "never
//! changes the answer, only the cost" property readable.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;
use super::eval::Evaluation;
use super::nodetest::CompiledTest;
use crate::err_setf;
use crate::ptr_table::PtrTable;
use crate::token::Token;

/// Is the context exactly the document node? Both index fast paths need that:
/// `descendant::tag` from the document is precisely "every element named tag",
/// which is what the index groups.
///
fn context_is_document<'e, 'd, D: Dom<'d>>(doc: D, set: &NodeSet<D::Node>) -> bool {
    matches!(set.as_slice(), [n] if *n == doc.document_node())
}

/// `//tag` from the index instead of a tree walk: the step's result, or None
/// when the shape does not qualify.
pub fn try_descendant_index<'e, 'd, D: Dom<'d>>(
    doc: D,
    ct: &CompiledTest<'_>,
    context_set: &NodeSet<D::Node>,
    budget: &mut Budget,
) -> Result<Option<NodeSet<D::Node>>, Reported> {
    let Some(local) = ct.name() else {
        return Ok(None);
    };
    if ct.axis() != Axis::Descendant || !context_is_document::<D>(doc, context_set) {
        return Ok(None);
    }
    let Some(bucket) = doc.name_bucket(local, ct.uri()) else {
        return Ok(None);
    };
    let mut result = NodeSet::new();
    for &n in bucket.nodes {
        budget.charge_op()?;
        if bucket.recheck && !ct.matches(doc, n) {
            continue;
        }
        result.push(n, budget)?;
    }
    Ok(Some(result))
}

/// `//name[N]` - the two leading steps `descendant-or-self::node()` and
/// `child::name[N]`, rooted at the document - selects, for every node, its Nth
/// name-child. That is NOT `(//name)[N]` and not `descendant::name[N]`.
///
/// The index lists matching elements in document order, so a parent's
/// name-children appear among them in child order: one sweep with a
/// pointer-keyed parent -> count map emits exactly those whose running count
/// reaches N, already in document order, with no sort or dedup.
///
/// The shape's N, and the name test's local name.
fn nth_shape<'s, 'e, 'd, D: Dom<'d>>(
    doc: D,
    s0: &Step,
    s1: &'s Step,
    seed: &NodeSet<D::Node>,
) -> Option<(usize, &'s [u8])> {
    if s0.axis != Axis::DescendantOrSelf
        || !matches!(s0.test, NodeTest::Node)
        || !s0.predicates.is_empty()
    {
        return None;
    }
    let NodeTest::Name { local, .. } = &s1.test else {
        return None;
    };
    if s1.axis != Axis::Child {
        return None;
    }
    /* The sole predicate must be a bare positive-integer literal, which is
     * position() == N. `[position()=N]` and `[last()]` are binops or calls and
     * fall back. */
    let [pred] = s1.predicates.as_slice() else {
        return None;
    };
    let ExprKind::LiteralNum(dn) = pred.kind else {
        return None;
    };
    /* NaN is spelled out rather than left to a negated comparison: `[NaN]`
     * must fall back, and `!(dn >= 1.0)` says so only by accident. */
    if dn.is_nan() || dn < 1.0 || dn != dn.trunc() || dn > usize::MAX as f64 {
        return None;
    }
    if !context_is_document::<D>(doc, seed) {
        return None;
    }
    Some((dn as usize, local))
}

/// `//name[N]` from the index: the two steps' result, or None when the shape
/// or the index cannot serve it - a sweep abandoned midway included, so a
/// partly filled set never reaches the caller.
pub fn try_descendant_index_nth<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    s0: &Step,
    s1: &Step,
    seed: &NodeSet<D::Node>,
) -> Result<Option<NodeSet<D::Node>>, Reported> {
    let doc = ev.doc;
    let Some((need, local)) = nth_shape::<D>(doc, s0, s1, seed) else {
        return Ok(None);
    };
    let names: &'e Names = ev.names;
    let ct = CompiledTest::new(&s1.test, s1.axis, names, ev.cx.lax(), ev.budget.sink())?;
    let Some(bucket) = doc.name_bucket(local, ct.uri()) else {
        return Ok(None);
    };
    let mut result = NodeSet::new();
    if bucket.nodes.is_empty() {
        return Ok(Some(result));
    }

    /* A count per parent, sized for one parent per element - a table that
     * never grows, so a full one is the tree changing under us: fall back. */
    let Some(mut per_parent) = PtrTable::<Token, usize>::with_keys(bucket.nodes.len(), 0) else {
        return Err(err_setf!(
            ev.budget.sink(),
            ErrorKind::Oom,
            "out of memory (//name[N])"
        ));
    };
    let budget = &mut ev.budget;

    for &e in bucket.nodes {
        budget.charge_op()?;
        if bucket.recheck && !ct.matches(doc, e) {
            continue;
        }
        /* An element of the bucket always has a parent; a parentless one would
         * be the index disagreeing with the tree, and the walk answers then. */
        let Some(par) = doc.parent(e) else {
            return Ok(None);
        };
        let Some(slot) = per_parent.insert(D::token(par), 0) else {
            return Ok(None);
        };
        let count = per_parent.slot_mut(slot);
        *count += 1;
        if *count == need {
            result.push(e, budget)?;
        }
    }
    Ok(Some(result))
}
