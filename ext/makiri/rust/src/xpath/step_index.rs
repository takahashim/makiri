//! The two element-index fast paths: `//tag` and `//tag[N]`.
//!
//! Both answer a document-rooted descendant name test from the document's
//! element index instead of walking the tree, and both are pure optimisation -
//! each returns Ok(false) whenever the shape or the index cannot serve it, and
//! the caller walks. Keeping them out of the step driver keeps that "never
//! changes the answer, only the cost" property readable.

use super::abi::*;
use super::ast_view::step_preds;
use super::dom::*;
use super::msg::Bytes;
use super::nodetest::{lookup_ns, node_principal_match, Bindings};
use super::own::Set;
use super::value::owned_bytes;
use crate::err_setf;
use crate::falloc::Reserve;
use core::ffi::c_void;
use core::ptr;

/// Is the context exactly the document node? Both index fast paths need that:
/// `descendant::tag` from the document is precisely "every element named tag",
/// which is what the index groups.
unsafe fn context_is_document<D: Dom>(ctx: *mut Context, set: &Set) -> bool {
    if set.count() != 1 {
        return false;
    }
    let dh = mkr_ctx_document(ctx);
    if dh.is_null() {
        return false;
    }
    set.get::<D>(0) == D::document_node(D::doc_from_void(dh))
}

/// `//tag` from the index instead of a tree walk. Returns Ok(true) when it
/// filled `result`, Ok(false) when the shape does not qualify.
///
/// # Safety
/// `step` must be a live step of the AST being evaluated, `context_set` hold
/// live handles, and `b` be bindings built for this context.
pub unsafe fn try_descendant_index<D: Dom>(
    doc: D::Doc,
    step: *const Step,
    context_set: &Set,
    result: &mut Set,
    b: &Bindings<D>,
    err: *mut Error,
) -> Result<bool, Reported> {
    let test = &raw const (*step).test;
    if (*step).axis != AXIS_DESCENDANT
        || (*test).kind != NT_NAME
        || (*test).local.is_absent()
        || !context_is_document::<D>(b.ctx, context_set)
    {
        return Ok(false);
    }
    let ns_uri = if (*test).prefix.is_absent() {
        None
    } else {
        b.pre
    };
    if (*test).prefix.is_present() && ns_uri.is_none() {
        return Ok(false); /* eval_step pre-resolves, so this should not happen */
    }
    let bucket = match D::name_bucket(b.ctx, owned_bytes((*test).local), ns_uri, b.lax) {
        Some(bk) => bk,
        None => return Ok(false),
    };
    let limits = mkr_ctx_limits(b.ctx);
    for &p in bucket.nodes {
        mkr_limit_eval_op(limits, err)?;
        let n = D::from_void(p);
        if bucket.recheck && !node_principal_match::<D>(doc, test, n, (*step).axis, b) {
            continue;
        }
        result.push::<D>(n, limits, err)?;
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
unsafe fn nth_shape<D: Dom>(
    ctx: *mut Context,
    s0: *const Step,
    s1: *const Step,
    seed: &Set,
) -> Option<usize> {
    if (*s0).axis != AXIS_DESCENDANT_OR_SELF
        || (*s0).test.kind != NT_NODE
        || (*s0).test.prefix.is_present()
        || (*s0).npredicates != 0
    {
        return None;
    }
    if (*s1).axis != AXIS_CHILD
        || (*s1).test.kind != NT_NAME
        || (*s1).test.local.is_absent()
        || (*s1).npredicates != 1
    {
        return None;
    }
    /* The sole predicate must be a bare positive-integer literal, which is
     * position() == N. `[position()=N]` and `[last()]` are binops or calls and
     * fall back. */
    let pred = step_preds(s1)[0];
    if pred.is_null() || (*pred).kind != NK_LITERAL_NUM {
        return None;
    }
    let dn = (*pred).u.literal_num;
    /* NaN is spelled out rather than left to a negated comparison: `[NaN]`
     * must fall back, and `!(dn >= 1.0)` says so only by accident. */
    if dn.is_nan() || dn < 1.0 || dn != dn.trunc() || dn > usize::MAX as f64 {
        return None;
    }
    if !context_is_document::<D>(ctx, seed) {
        return None;
    }
    Some(dn as usize)
}

///
/// # Safety
/// Same as `try_descendant_index`, for the two leading steps `s0` and `s1`.
pub unsafe fn try_descendant_index_nth<D: Dom>(
    ctx: *mut Context,
    s0: *const Step,
    s1: *const Step,
    seed: &Set,
    result: &mut Set,
    err: *mut Error,
) -> Result<bool, Reported> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let need = match nth_shape::<D>(ctx, s0, s1, seed) {
        Some(n) => n,
        None => return Ok(false),
    };
    let test = &raw const (*s1).test;
    let ns_uri: Option<&[u8]> = if (*test).prefix.is_absent() {
        None
    } else {
        match lookup_ns(ctx, owned_bytes((*test).prefix)) {
            Some(u) => Some(u),
            None => {
                return Err(err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "unknown namespace prefix '{}' in name test",
                    Bytes(owned_bytes((*test).prefix))
                ));
            }
        }
    };
    let b = Bindings::<D>::new(ctx, ns_uri);
    let bucket = match D::name_bucket(ctx, owned_bytes((*test).local), ns_uri, b.lax) {
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
    let mut tab: Vec<(*const c_void, usize)> = Vec::new();
    if tab.mkr_reserve_exact(cap).is_err() {
        return Err(err_setf!(err, XP_ERR_OOM, "out of memory (//name[N])"));
    }
    tab.resize(cap, (ptr::null(), 0));
    let mask = cap - 1;
    let limits = mkr_ctx_limits(ctx);

    for &p in bucket.nodes {
        mkr_limit_eval_op(limits, err)?;
        let e = D::from_void(p);
        if bucket.recheck && !node_principal_match::<D>(doc, test, e, (*s1).axis, &b) {
            continue;
        }
        let par = D::to_void(D::parent(doc, e)) as *const c_void;
        let mut h = (ptr_hash(par) as usize) & mask;
        while !tab[h].0.is_null() && tab[h].0 != par {
            h = (h + 1) & mask;
        }
        tab[h].0 = par;
        tab[h].1 += 1;
        if tab[h].1 == need {
            result.push::<D>(e, limits, err)?;
        }
    }
    Ok(true)
}
