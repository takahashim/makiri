//! The two element-index fast paths: `//tag` and `//tag[N]`.
//!
//! Both answer a document-rooted descendant name test from the document's
//! element index instead of walking the tree, and both are pure optimisation -
//! each returns Ok(false) whenever the shape or the index cannot serve it, and
//! the caller walks. Keeping them out of the step driver keeps that "never
//! changes the answer, only the cost" property readable.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;
use super::eval::Evaluation;
use super::msg::Bytes;
use super::nodetest::{node_principal_match, Bindings};
use super::token::Token;
use crate::err_setf;
use crate::falloc::Reserve;

/// Is the context exactly the document node? Both index fast paths need that:
/// `descendant::tag` from the document is precisely "every element named tag",
/// which is what the index groups.
///
fn context_is_document<'e, 'd, D: Dom<'d>>(doc: D, set: &NodeSet<D::Node>) -> bool {
    set.len() == 1 && set.get(0) == doc.document_node()
}

/// `//tag` from the index instead of a tree walk. Returns Ok(true) when it
/// filled `result`, Ok(false) when the shape does not qualify.
pub fn try_descendant_index<'e, 'd, D: Dom<'d>>(
    doc: D,
    step: &Step,
    context_set: &NodeSet<D::Node>,
    result: &mut NodeSet<D::Node>,
    b: &Bindings<'e, 'd, D>,
    budget: &mut Budget,
) -> Result<bool, Reported> {
    let test = &step.test;
    let Some(local) = test.local.as_deref() else {
        return Ok(false);
    };
    if step.axis != Axis::Descendant
        || test.kind != TestKind::Name
        || !context_is_document::<D>(doc, context_set)
    {
        return Ok(false);
    }
    let ns_uri = if test.prefix.is_none() { None } else { b.pre };
    if test.prefix.is_some() && ns_uri.is_none() {
        return Ok(false); /* eval_step pre-resolves, so this should not happen */
    }
    let bucket = match doc.name_bucket(local, ns_uri, b.lax) {
        Some(bk) => bk,
        None => return Ok(false),
    };
    for &n in bucket.nodes {
        budget.charge_op()?;
        if bucket.recheck && !node_principal_match::<D>(doc, test, n, step.axis, b) {
            continue;
        }
        result.push(n, budget)?;
    }
    Ok(true)
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
fn nth_shape<'e, 'd, D: Dom<'d>>(
    doc: D,
    s0: &Step,
    s1: &Step,
    seed: &NodeSet<D::Node>,
) -> Option<usize> {
    if s0.axis != Axis::DescendantOrSelf
        || s0.test.kind != TestKind::Node
        || s0.test.prefix.is_some()
        || !s0.predicates.is_empty()
    {
        return None;
    }
    if s1.axis != Axis::Child || s1.test.kind != TestKind::Name || s1.test.local.is_none() {
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
    Some(dn as usize)
}

pub fn try_descendant_index_nth<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    s0: &Step,
    s1: &Step,
    seed: &NodeSet<D::Node>,
    result: &mut NodeSet<D::Node>,
) -> Result<bool, Reported> {
    let err = ev.budget.sink();
    let doc = ev.doc;
    let cx = ev.cx;
    let names = ev.names;
    let need = match nth_shape::<D>(doc, s0, s1, seed) {
        Some(n) => n,
        None => return Ok(false),
    };
    let test = &s1.test;
    let ns_uri: Option<&[u8]> = match test.prefix.as_deref() {
        None => None,
        Some(prefix) => match names.lookup_ns(prefix) {
            Some(u) => Some(u),
            None => {
                return Err(err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "unknown namespace prefix '{}' in name test",
                    Bytes(prefix)
                ));
            }
        },
    };
    let b = Bindings::<D>::new(cx, names, doc, ns_uri);
    let local = test.local.as_deref().unwrap_or(&[]);
    let bucket = match doc.name_bucket(local, ns_uri, b.lax) {
        Some(bk) => bk,
        None => return Ok(false),
    };
    if bucket.nodes.is_empty() {
        return Ok(true);
    }

    /* A pointer-keyed count per parent. Sized from the bucket so the open
     * addressing stays under a 2/3 load; an overflow in the sizer falls back to
     * the generic evaluator rather than risking a table that never finds a slot. */
    let want = bucket.nodes.len() + (bucket.nodes.len() >> 1) + 1;
    let Some(cap) = want.checked_next_power_of_two() else {
        return Ok(false);
    };
    let mut tab: Vec<(Token, usize)> = Vec::new();
    if tab.mkr_reserve_exact(cap).is_err() {
        return Err(err_setf!(err, XP_ERR_OOM, "out of memory (//name[N])"));
    }
    tab.resize(cap, (Token::null(), 0));
    let mask = cap - 1;
    let budget = &mut ev.budget;

    for &e in bucket.nodes {
        budget.charge_op()?;
        if bucket.recheck && !node_principal_match::<D>(doc, test, e, s1.axis, &b) {
            continue;
        }
        let par = doc
            .parent(e)
            .map_or(Token::null(), D::token);
        let mut h = (ptr_hash(par.as_ptr() as *const u8) as usize) & mask;
        while !tab[h].0.is_null() && tab[h].0 != par {
            h = (h + 1) & mask;
        }
        tab[h].0 = par;
        tab[h].1 += 1;
        if tab[h].1 == need {
            result.push(e, budget)?;
        }
    }
    Ok(true)
}
