//! The XPath 1.0 evaluator (mkr_xpath_eval_body.h): axis walks, node tests,
//! predicates, the operator semantics, and the two index fast paths.
//!
//! Generic over `Dom`, so one body compiles per representation - what the C
//! did by `#include`-ing this file twice behind different macros.

/* A failure is `Err(Reported)`: the detail lives in the `*mut Error` the caller
 * owns, exactly as it does in the C, and the proof says it was written. */

use super::abi::*;
use super::ast_view::{path_steps, step_preds};
use super::attr_pred::{attr_pred_matches, match_attr_pred};
use super::axis::{axis_can_alias, axis_is_implemented, axis_name, is_reverse_axis, walk_axis};
use super::dom::*;
use super::funcs;
use super::msg::Bytes;
use super::nodetest::{lookup_ns, node_principal_match, Bindings};
use super::order::nodeset_unique_sorted;
use super::own::{OwnedText, OwnedVal, Set};
use super::step_index::{try_descendant_index, try_descendant_index_nth};
use super::value::*;
use crate::err_setf;
use crate::falloc::Reserve;
use core::ffi::{c_char, c_void};
use core::ptr;

/// An evaluation step: the value, or proof its error was written to `err`.
type EvalResult<T = ()> = Result<T, Reported>;

/* ---------- predicates ---------- */

unsafe fn apply_predicates<D: Dom>(
    ctx: *mut Context,
    preds: &[*mut Node],
    inout: &mut Set,
    err: ErrSink,
) -> EvalResult {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let limits = mkr_ctx_limits(ctx);
    for &pred in preds {
        let mut kept = Set::new();

        /* Specialise [@name] / [@name='lit'] - position-independent, so applying
         * it per predicate even amid others matches the generic path. */
        if let Some(ap) = match_attr_pred(pred) {
            for i in 0..inout.count() {
                /* Charge per candidate: this replaces a per-node generic
                 * predicate eval, which would tick through eval_node, so the
                 * shortcut stays under the same budget as the path it skips. */
                mkr_limit_eval_op(limits, err)?;
                let n = inout.get::<D>(i);
                if attr_pred_matches::<D>(doc, &ap, n) {
                    kept.push::<D>(n, limits, err)?;
                }
            }
            inout.replace(kept.take());
            continue;
        }

        let size = inout.count();
        for i in 0..size {
            let n = inout.get::<D>(i);
            let mut v = OwnedVal::new();
            let pf = Focus::<D> {
                node: n,
                pos: i + 1,
                size,
            };
            eval_node::<D>(ctx, pred, &pf, v.as_mut(), err)?;
            /* A bare number predicate means position() = that number. */
            let keep = if (*v.as_ptr()).type_ == T_NUMBER {
                (*v.as_ptr()).u.number == (i + 1) as f64
            } else {
                val_to_boolean(v.as_ptr())
            };
            if keep {
                kept.push::<D>(n, limits, err)?;
            }
        }
        inout.replace(kept.take());
    }
    Ok(())
}

/* ---------- steps ---------- */

unsafe fn eval_step<D: Dom>(
    ctx: *mut Context,
    step: *const Step,
    context_set: &Set,
    out: &mut Set,
    err: ErrSink,
) -> EvalResult {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let axis = (*step).axis;
    if !axis_is_implemented(axis) {
        return Err(err_setf!(
            err,
            XP_ERR_NOT_IMPLEMENTED,
            "native engine: axis '{}' not implemented yet",
            axis_name(axis)
        ));
    }
    let test = &raw const (*step).test;
    let limits = mkr_ctx_limits(ctx);

    /* Resolve the namespace prefix once up front (covering `prefix:local` and
     * `prefix:*`): a uniform RUNTIME error rather than a silently empty match,
     * and every per-node match then reuses the URI instead of re-resolving.
     *
     * The borrow lives in the context's registry and cannot be freed mid-
     * evaluate: the only path that frees it is re-registering the same prefix,
     * and the glue refuses register_namespace (and register_variable, node=)
     * while an evaluate is in progress on this context - which is exactly when a
     * predicate handler could re-enter. */
    let pre: Option<&[u8]> = if (*test).prefix.is_absent() {
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

    let b = Bindings::<D>::new(ctx, pre);

    /* A post-pass (sort to document order, then optional adjacent dedup) is
     * needed when the axis emits in reverse order per context, when it aliases
     * across contexts, or when several contexts produce results that interleave.
     * child is the canonical third case: html's children are head and body, and
     * head's children include title, so naive concatenation gives
     * [head, body, title] where document order is [head, title, body]. Only self
     * and attribute stay in order under concatenation. */
    let need_post_pass = is_reverse_axis(axis)
        || (axis_can_alias(axis) && context_set.count() > 1)
        || (context_set.count() > 1 && axis != AXIS_SELF && axis != AXIS_ATTRIBUTE);

    let mut result = Set::new();

    let preds = step_preds(step);
    if preds.is_empty() {
        if !try_descendant_index::<D>(doc, step, context_set, &mut result, &b, err)? {
            /* No-predicate walk: every context goes straight into the result
             * buffer regardless of the post-pass, saving the per-context
             * fragment the predicate path needs. */
            let mut failure: Option<Reported> = None;
            for ci in 0..context_set.count() {
                let mut visit = |n: D::Node| -> bool {
                    /* Charge every visited node. The axis walk is the dominant
                     * work of a step, and a low-selectivity walk name-tests many
                     * nodes while pushing few - so without this the node-set
                     * cap, which bounds only what is pushed, leaves the walk
                     * itself bounded by document size, defeating max_eval_ops on
                     * a descendant walk that matches nothing. */
                    if let Err(e) = mkr_limit_eval_op(limits, err) {
                        failure = Some(e);
                        return true;
                    }
                    if node_principal_match::<D>(doc, test, n, axis, &b) {
                        if let Err(e) = result.push::<D>(n, limits, err) {
                            failure = Some(e);
                            return true;
                        }
                    }
                    false
                };
                walk_axis::<D, _>(doc, axis, context_set.get::<D>(ci), &mut visit);
                if let Some(e) = failure.take() {
                    return Err(e);
                }
            }
        }
    } else {
        /* Predicate path: position() and last() are per-context, so each
         * context's fragment has to be materialised before filtering. One
         * fragment buffer is reused across iterations, so its storage grows to
         * the largest single-context cardinality once rather than per iteration. */
        let mut fragment = Set::new();
        for ci in 0..context_set.count() {
            fragment.0.count = 0;
            let mut failure: Option<Reported> = None;
            {
                let frag = &mut fragment;
                let mut visit = |n: D::Node| -> bool {
                    if let Err(e) = mkr_limit_eval_op(limits, err) {
                        failure = Some(e);
                        return true;
                    }
                    if node_principal_match::<D>(doc, test, n, axis, &b) {
                        if let Err(e) = frag.push::<D>(n, limits, err) {
                            failure = Some(e);
                            return true;
                        }
                    }
                    false
                };
                walk_axis::<D, _>(doc, axis, context_set.get::<D>(ci), &mut visit);
            }
            if let Some(e) = failure {
                return Err(e);
            }

            /* Predicates apply per context with axis-natural position numbering
             * (§2.4). For a reverse axis the fragment is in reverse-document
             * order, so [1] is the closest to the context - the intended
             * meaning. */
            apply_predicates::<D>(ctx, preds, &mut fragment, err)?;
            for i in 0..fragment.count() {
                result.push::<D>(fragment.get::<D>(i), limits, err)?;
            }
        }
    }

    if need_post_pass && result.count() > 1 {
        nodeset_unique_sorted::<D>(ctx, result.as_mut());
    }
    out.replace(result.take());
    Ok(())
}

unsafe fn eval_steps<D: Dom>(
    ctx: *mut Context,
    steps: &[Step],
    seed: &mut Set,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let mut current = Set::adopt(seed.take());
    let mut rest = steps;

    if let [s0, s1, ..] = steps {
        let mut nth = Set::new();
        if try_descendant_index_nth::<D>(ctx, s0, s1, &current, &mut nth, err)? {
            current = Set::adopt(nth.take());
            rest = &steps[2..];
        }
    }
    for step in rest {
        let mut next = Set::new();
        eval_step::<D>(ctx, step, &current, &mut next, err)?;
        current = Set::adopt(next.take());
    }
    (*out).type_ = T_NODESET;
    (*out).u.nodeset = current.take();
    Ok(())
}

/* ---------- comparisons ---------- */

/// §3.4 equality. A node-set on either side means "true iff SOME node satisfies
/// it"; all node string-values go through the per-evaluate cache, so an M-by-N
/// comparison costs O(M+N) string builds.
unsafe fn compare_eq<D: Dom>(
    ctx: *mut Context,
    l: *const Val,
    r: *const Val,
    op: u32,
    err: ErrSink,
) -> EvalResult<bool> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let limits = mkr_ctx_limits(ctx);
    let want_eq = op == OP_EQ;
    let (lt, rt) = ((*l).type_, (*r).type_);

    if lt == T_NODESET && rt == T_NODESET {
        /* The pair scan itself is M*N even though the string builds are O(M+N),
         * so charge each pair: otherwise an all-pairs node-set equality drives
         * up to ~1e14 comparisons as a handful of ops. */
        let (ls, rs) = (&raw const (*l).u.nodeset, &raw const (*r).u.nodeset);
        for i in 0..(*ls).count {
            let a = cached_node_text::<D>(ctx, nodeset_at::<D>(ls, i), err)?;
            for j in 0..(*rs).count {
                mkr_limit_eval_op(limits, err)?;
                let b = cached_node_text::<D>(ctx, nodeset_at::<D>(rs, j), err)?;
                if (a == b) == want_eq {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    if lt == T_NODESET || rt == T_NODESET {
        let (ns, sc) = if lt == T_NODESET { (l, r) } else { (r, l) };
        let set = &raw const (*ns).u.nodeset;
        match (*sc).type_ {
            T_NUMBER => {
                let target = (*sc).u.number;
                for i in 0..(*set).count {
                    mkr_limit_eval_op(limits, err)?;
                    let s = cached_node_text::<D>(ctx, nodeset_at::<D>(set, i), err)?;
                    if (bytes_to_number(s) == target) == want_eq {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            T_BOOLEAN => {
                let eq = ((*set).count > 0) == ((*sc).u.boolean != 0);
                Ok(if want_eq { eq } else { !eq })
            }
            _ => {
                let mut target = OwnedText::new();
                val_to_owned_text_or_fail::<D>(doc, sc, limits, err, target.as_mut())?;
                let want = target.as_slice();
                for i in 0..(*set).count {
                    mkr_limit_eval_op(limits, err)?;
                    let s = cached_node_text::<D>(ctx, nodeset_at::<D>(set, i), err)?;
                    if (s == want) == want_eq {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    } else if lt == T_BOOLEAN || rt == T_BOOLEAN {
        let eq = val_to_boolean(l) == val_to_boolean(r);
        Ok(if want_eq { eq } else { !eq })
    } else if lt == T_NUMBER || rt == T_NUMBER {
        /* Both operands are non-node-sets here, so the unchecked coercion is the
         * right entry - it cannot allocate. */
        let eq = val_to_number_unchecked::<D>(doc, l) == val_to_number_unchecked::<D>(doc, r);
        Ok(if want_eq { eq } else { !eq })
    } else {
        let mut ls = OwnedText::new();
        let mut rs = OwnedText::new();
        val_to_owned_text_or_fail::<D>(doc, l, limits, err, ls.as_mut())?;
        val_to_owned_text_or_fail::<D>(doc, r, limits, err, rs.as_mut())?;
        let eq = ls.as_slice() == rs.as_slice();
        Ok(if want_eq { eq } else { !eq })
    }
}

fn rel_hit(op: u32, a: f64, b: f64) -> bool {
    match op {
        OP_LT => a < b,
        OP_LE => a <= b,
        OP_GT => a > b,
        OP_GE => a >= b,
        _ => false,
    }
}

/// §3.4 relational. A node-set on either side is true iff SOME pair satisfies
/// the relation on their numeric string-values - every pair, not just the first
/// node of each side.
unsafe fn compare_rel<D: Dom>(
    ctx: *mut Context,
    l: *const Val,
    r: *const Val,
    op: u32,
    err: ErrSink,
) -> EvalResult<bool> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let limits = mkr_ctx_limits(ctx);
    let (lt, rt) = ((*l).type_, (*r).type_);

    if lt == T_NODESET && rt == T_NODESET {
        let (ls, rs) = (&raw const (*l).u.nodeset, &raw const (*r).u.nodeset);
        for i in 0..(*ls).count {
            let a = bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(ls, i), err)?);
            for j in 0..(*rs).count {
                mkr_limit_eval_op(limits, err)?;
                let b = bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(rs, j), err)?);
                if rel_hit(op, a, b) {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    if lt == T_NODESET || rt == T_NODESET {
        let (ns, sc) = if lt == T_NODESET { (l, r) } else { (r, l) };
        let swap = lt != T_NODESET;
        let mut scn = 0.0;
        val_to_number_or_fail::<D>(doc, sc, limits, err, &mut scn)?;
        let set = &raw const (*ns).u.nodeset;
        for i in 0..(*set).count {
            mkr_limit_eval_op(limits, err)?;
            let nv = bytes_to_number(cached_node_text::<D>(ctx, nodeset_at::<D>(set, i), err)?);
            let (a, b) = if swap { (scn, nv) } else { (nv, scn) };
            if rel_hit(op, a, b) {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let (mut a, mut b) = (0.0, 0.0);
    val_to_number_or_fail::<D>(doc, l, limits, err, &mut a)?;
    val_to_number_or_fail::<D>(doc, r, limits, err, &mut b)?;
    Ok(rel_hit(op, a, b))
}

/* ---------- union ---------- */

unsafe fn union_nodeset<D: Dom>(
    ctx: *mut Context,
    l: *const Val,
    r: *const Val,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    if (*l).type_ != T_NODESET || (*r).type_ != T_NODESET {
        return Err(err_setf!(
            err,
            XP_ERR_TYPE,
            "operands of '|' must be node-sets"
        ));
    }
    let limits = mkr_ctx_limits(ctx);
    /* Push both sides without deduplicating per insert - that was quadratic -
     * then sort once and collapse adjacent duplicates. */
    let mut merged = Set::new();
    for side in [l, r] {
        let set = &raw const (*side).u.nodeset;
        for i in 0..(*set).count {
            merged.push::<D>(nodeset_at::<D>(set, i), limits, err)?;
        }
    }
    /* §3.3: the result of '|' is a node-set in document order, which the
     * downstream string() / number() / positional predicates assume. */
    nodeset_unique_sorted::<D>(ctx, merged.as_mut());
    (*out).type_ = T_NODESET;
    (*out).u.nodeset = merged.take();
    Ok(())
}

/* ---------- the at_xpath first-match short-circuit ---------- */

/// `Node#at_xpath` wants only the first node in document order, and today builds
/// the whole node-set to take [0]. For the common "find a descendant by name
/// (plus a simple attribute predicate)" shapes the subtree can be walked in
/// document order and stopped at the first match - the XPath-side analogue of
/// at_css's MATCH_FIRST.
///
/// Recognised, after the parser's `//` peephole:
///
/// ```text
/// //X        .//X          -> PATH [ {descendant, X} ]
/// //X[@a..]  .//X[@a..]    -> PATH [ {desc-or-self, node()}, {child, X, preds} ]
/// descendant::X[@a..]      -> PATH [ {descendant, X, preds} ]
/// ```
///
/// where every predicate is a position-independent `[@name]` / `[@name='lit']`.
/// Each denotes "the strict descendants of the start node matching the test and
/// predicates, in document order", so the first node the pre-order walk reaches
/// IS node-set[0] of the full evaluation - identical, just without building the
/// rest. Anything else returns None and the caller runs the full evaluator.
unsafe fn first_recognise(ast: *const Node) -> Option<*const Step> {
    if ast.is_null() || (*ast).kind != NK_PATH {
        return None;
    }
    let steps = (*ast).u.path.steps;
    let nsteps = (*ast).u.path.nsteps;
    let nt: *const Step = if nsteps == 1 && (*steps).axis == AXIS_DESCENDANT {
        steps
    } else if nsteps == 2
        && (*steps).axis == AXIS_DESCENDANT_OR_SELF
        && (*steps).test.kind == NT_NODE
        && (*steps).npredicates == 0
        && (*steps.add(1)).axis == AXIS_CHILD
    {
        steps.add(1)
    } else {
        return None;
    };
    /* A prefixed name test is allowed - the caller reproduces the step driver's
     * "unknown prefix is a RUNTIME error" first, and the name match resolves the
     * prefix exactly as the full evaluator does. A prefixed ATTRIBUTE predicate
     * still falls back: match_attr_step requires an unprefixed @name. */
    for &p in step_preds(nt) {
        match_attr_pred(p)?;
    }
    Some(nt)
}

/// Does `n` satisfy every already-recognised attribute predicate of `step`?
unsafe fn first_node_ok<D: Dom>(doc: D::Doc, step: *const Step, n: D::Node) -> bool {
    for &p in step_preds(step) {
        /* The recogniser already confirmed the shape. */
        let ap = match match_attr_pred(p) {
            Some(ap) => ap,
            None => return false,
        };
        if !attr_pred_matches::<D>(doc, &ap, n) {
            return false;
        }
    }
    true
}

/// Walk for the first match if `ast` is a recognised shape.
///
/// Returns Ok(Some(node)) or Ok(Some(null)) when it handled the expression,
/// Ok(None) when the shape is not recognised, and Err when the op budget was
/// exceeded. Every visited node is charged, so a huge late- or no-match document
/// fails closed here exactly as it would in the full evaluator.
///
/// # Safety
/// `ctx` must be the evaluating context and `ast` a live AST.
pub unsafe fn try_first_match<D: Dom>(
    ctx: *mut Context,
    ast: *const Node,
    err: ErrSink,
) -> Result<Option<D::Node>, Reported> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let step = match first_recognise(ast) {
        Some(s) => s,
        None => return Ok(None),
    };
    let test = &raw const (*step).test;

    /* Reproduce the step driver's prefix validation, so the fast path stays
     * identical to the full evaluator down to the errors - and keep what it
     * resolved, so the walk below does not look the prefix up again per node. */
    let pre = if (*test).prefix.is_absent() {
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

    let start: D::Node = if (*ast).u.path.absolute != 0 {
        D::document_node(D::doc_from_void(mkr_ctx_document(ctx)))
    } else {
        D::from_void(mkr_ctx_node(ctx))
    };
    if D::is_null(start) {
        return Ok(Some(D::null())); /* recognised; no context means no match */
    }

    let limits = mkr_ctx_limits(ctx);
    let b = Bindings::<D>::new(ctx, pre);
    let mut n = D::first_child(doc, start);
    while !D::is_null(n) {
        mkr_limit_eval_op(limits, err)?;
        if node_principal_match::<D>(doc, test, n, (*step).axis, &b)
            && first_node_ok::<D>(doc, step, n)
        {
            return Ok(Some(n));
        }
        if !D::is_null(D::first_child(doc, n)) {
            n = D::first_child(doc, n);
            continue;
        }
        while n != start && D::is_null(D::next(doc, n)) {
            n = D::parent(doc, n);
        }
        if n == start {
            break;
        }
        n = D::next(doc, n);
    }
    Ok(Some(D::null()))
}

/* ---------- the expression evaluator ---------- */

unsafe fn eval_path<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    self_node: D::Node,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let limits = mkr_ctx_limits(ctx);
    let mut seed = Set::new();
    if (*n).u.path.absolute != 0 {
        let root_h = mkr_ctx_document(ctx);
        if root_h.is_null() {
            return Err(err_setf!(
                err,
                XP_ERR_RUNTIME,
                "absolute path with no document"
            ));
        }
        let root = D::document_node(D::doc_from_void(root_h));
        seed.push::<D>(root, limits, err)?;
    } else {
        seed.push::<D>(self_node, limits, err)?;
    }
    eval_steps::<D>(
        ctx,
        path_steps((*n).u.path.steps, (*n).u.path.nsteps),
        &mut seed,
        out,
        err,
    )
}

unsafe fn eval_filter<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let f = &raw const (*n).u.filter;
    let mut primary = OwnedVal::new();
    eval_node::<D>(ctx, (*f).expr, focus, primary.as_mut(), err)?;
    if (*f).npreds > 0 {
        if (*primary.as_ptr()).type_ != T_NODESET {
            return Err(err_setf!(
                err,
                XP_ERR_TYPE,
                "predicate applied to non-node-set"
            ));
        }
        let mut set = Set::adopt((*primary.as_ptr()).u.nodeset);
        (*primary.as_mut()).u.nodeset = NodeSet {
            items: ptr::null_mut(),
            count: 0,
            capacity: 0,
        };
        let preds = core::slice::from_raw_parts((*f).preds, (*f).npreds);
        apply_predicates::<D>(ctx, preds, &mut set, err)?;
        (*primary.as_mut()).u.nodeset = set.take();
    }
    if (*f).npath > 0 {
        if (*primary.as_ptr()).type_ != T_NODESET {
            return Err(err_setf!(err, XP_ERR_TYPE, "path applied to non-node-set"));
        }
        let mut seed = Set::adopt((*primary.as_ptr()).u.nodeset);
        (*primary.as_mut()).u.nodeset = NodeSet {
            items: ptr::null_mut(),
            count: 0,
            capacity: 0,
        };
        return eval_steps::<D>(
            ctx,
            path_steps((*f).path_steps, (*f).npath),
            &mut seed,
            out,
            err,
        );
    }
    *out = primary.take();
    Ok(())
}

unsafe fn eval_fncall<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let call = &raw const (*n).u.fncall;
    let prefix = owned_bytes((*call).prefix);
    let name = owned_bytes((*call).name);

    let ns_uri: Option<&[u8]> = if (*call).prefix.is_absent() {
        None
    } else {
        match lookup_ns(ctx, prefix) {
            Some(u) => Some(u),
            None => {
                return Err(err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "unknown namespace prefix '{}'",
                    Bytes(prefix)
                ));
            }
        }
    };
    let builtin = funcs::lookup::<D>(ns_uri, name);

    /* The arguments are evaluated once and reused by either path. Their values
     * are owned here and cleared on the way out. */
    let nargs = (*call).nargs;
    let mut args: Vec<Val> = Vec::new();
    if nargs > 0 {
        if args.mkr_reserve_exact(nargs).is_err() {
            return Err(err_setf!(
                err,
                XP_ERR_OOM,
                "out of memory allocating function arguments"
            ));
        }
        for i in 0..nargs {
            let mut v = val_zero(T_NODESET);
            if let Err(e) = eval_node::<D>(ctx, *(*call).args.add(i), focus, &mut v, err) {
                mkr_val_clear(&mut v);
                clear_args(&mut args);
                return Err(e);
            }
            args.push(v);
        }
    }

    let result = if let Some(f) = builtin {
        f(ctx, focus, &args, out, err)
    } else {
        /* No built-in. Delegate to the per-call resolver, which the Ruby handler
         * bridge installs for the duration of evaluate(). */
        let resolved = match mkr_ctx_func_resolver(ctx) {
            Some(resolver) => resolver(
                mkr_xpath_get_user_data(ctx),
                ctx,
                D::to_void(focus.node),
                focus.pos,
                focus.size,
                ns_uri.map_or(ptr::null(), |u| u.as_ptr() as *const c_char),
                (*call).name.as_ptr(),
                /* NULL rather than a dangling pointer when there are none,
                 * which is what the C hands a resolver. */
                if nargs == 0 {
                    ptr::null_mut()
                } else {
                    args.as_mut_ptr() as *mut c_void
                },
                nargs,
                out as *mut c_void,
                err.as_raw(),
            ),
            None => 1, /* not found */
        };
        match resolved {
            0 => Ok(()),
            r if r > 0 => Err(err_setf!(
                err,
                XP_ERR_RUNTIME,
                "unknown function {}{}{}",
                Bytes(prefix),
                if (*call).prefix.is_absent() { "" } else { ":" },
                Bytes(name)
            )),
            /* A negative answer is the resolver's failure, which it reports
             * through `err` before returning - its status-only C signature is
             * the one place the proof has to be taken on trust. */
            _ => Err(Reported::assume_written()),
        }
    };
    clear_args(&mut args);
    result
}

unsafe fn clear_args(args: &mut Vec<Val>) {
    for v in args.iter_mut() {
        mkr_val_clear(v);
    }
    args.clear();
}

unsafe fn eval_binop<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let b = &raw const (*n).u.binop;
    let op = (*b).op;
    let limits = mkr_ctx_limits(ctx);

    /* and / or short-circuit. */
    if op == OP_OR || op == OP_AND {
        let mut l = OwnedVal::new();
        eval_node::<D>(ctx, (*b).lhs, focus, l.as_mut(), err)?;
        let lb = val_to_boolean(l.as_ptr());
        if (op == OP_OR && lb) || (op == OP_AND && !lb) {
            *out = val_boolean(lb);
            return Ok(());
        }
        let mut r = OwnedVal::new();
        eval_node::<D>(ctx, (*b).rhs, focus, r.as_mut(), err)?;
        *out = val_boolean(val_to_boolean(r.as_ptr()));
        return Ok(());
    }

    let mut l = OwnedVal::new();
    let mut r = OwnedVal::new();
    eval_node::<D>(ctx, (*b).lhs, focus, l.as_mut(), err)?;
    eval_node::<D>(ctx, (*b).rhs, focus, r.as_mut(), err)?;
    let (lp, rp) = (l.as_ptr(), r.as_ptr());

    match op {
        OP_EQ | OP_NE => {
            *out = val_boolean(compare_eq::<D>(ctx, lp, rp, op, err)?);
            Ok(())
        }
        OP_LT | OP_LE | OP_GT | OP_GE => {
            *out = val_boolean(compare_rel::<D>(ctx, lp, rp, op, err)?);
            Ok(())
        }
        OP_ADD | OP_SUB | OP_MUL | OP_DIV | OP_MOD => {
            let (mut a, mut c) = (0.0, 0.0);
            val_to_number_or_fail::<D>(doc, lp, limits, err, &mut a)?;
            val_to_number_or_fail::<D>(doc, rp, limits, err, &mut c)?;
            *out = val_number(match op {
                OP_ADD => a + c,
                OP_SUB => a - c,
                OP_MUL => a * c,
                OP_DIV => a / c,
                _ => libm_fmod(a, c),
            });
            Ok(())
        }
        /* union_nodeset reports its own typed error; do not overwrite it. */
        OP_UNION => union_nodeset::<D>(ctx, lp, rp, out, err),
        _ => Err(err_setf!(err, XP_ERR_INTERNAL, "unexpected binop")),
    }
}

/// Unary minus: the operand as a number, negated.
unsafe fn eval_negate<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let limits = mkr_ctx_limits(ctx);
    let mut v = OwnedVal::new();
    eval_node::<D>(ctx, (*n).u.unary.expr, focus, v.as_mut(), err)?;
    let mut d = 0.0;
    val_to_number_or_fail::<D>(doc, v.as_ptr(), limits, err, &mut d)?;
    *out = val_number(-d);
    Ok(())
}

/// A string result copied from `bytes`.
unsafe fn string_value(
    out: *mut Val,
    bytes: &[u8],
    err: ErrSink,
    what: &core::ffi::CStr,
) -> EvalResult {
    let mut text = TextSlot::empty();
    owned_copy(&mut text, bytes, err, what)?;
    mkr_val_set_owned_text(out, text);
    Ok(())
}

/// The evaluator's only recursive function, and therefore the whole of "AST
/// recursion is bounded": one op and one recursion level are charged on entry
/// and the level is released at the single exit. Keeping it single-exit is what
/// makes that balance locally checkable.
unsafe fn eval_node<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let limits = mkr_ctx_limits(ctx);
    mkr_limit_eval_op(limits, err)?;
    /* A refused entry is not counted, so returning here needs no release. */
    mkr_limit_recurse_enter(limits, err)?;
    let result = eval_node_inner::<D>(ctx, n, focus, out, err);
    mkr_limit_recurse_leave(limits);
    result
}

unsafe fn eval_node_inner<D: Dom>(
    ctx: *mut Context,
    n: *const Node,
    focus: &Focus<D>,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    /* Hoisting: a context-independent subtree already computed in this evaluate
     * comes back as a clone, which keeps ownership clean - clearing either copy
     * is safe. */
    if (*n).is_context_independent != 0 && (*n).memoized != 0 {
        return val_clone(&raw const (*n).memo_value, out, err);
    }

    let result = match (*n).kind {
        NK_LITERAL_STR => string_value(
            out,
            owned_bytes((*n).u.literal),
            err,
            c"out of memory copying literal",
        ),
        NK_LITERAL_NUM => {
            *out = val_number((*n).u.literal_num);
            Ok(())
        }
        NK_VARREF => {
            let v = &raw const (*n).u.varref;
            let prefix = if (*v).prefix.is_absent() {
                None
            } else {
                Some(owned_bytes((*v).prefix))
            };
            match mkr_ctx_lookup_variable_text(ctx, prefix, owned_bytes((*v).name)) {
                Some(bytes) => {
                    string_value(out, bytes, err, c"out of memory copying variable value")
                }
                None => Err(err_setf!(
                    err,
                    XP_ERR_RUNTIME,
                    "undefined variable ${}{}{}",
                    Bytes(owned_bytes((*v).prefix)),
                    if (*v).prefix.is_absent() { "" } else { ":" },
                    Bytes(owned_bytes((*v).name))
                )),
            }
        }
        NK_FNCALL => eval_fncall::<D>(ctx, n, focus, out, err),
        NK_UNARY => eval_negate::<D>(ctx, n, focus, out, err),
        NK_BINOP => eval_binop::<D>(ctx, n, focus, out, err),
        NK_PATH => eval_path::<D>(ctx, n, focus.node, out, err),
        NK_FILTER => eval_filter::<D>(ctx, n, focus, out, err),
        _ => Err(err_setf!(err, XP_ERR_INTERNAL, "unknown AST node")),
    };

    /* Memoize a context-independent subtree on success. The clone keeps the
     * caller's value independent of the cached one, which matters because the
     * caller is free to consume theirs. */
    if result.is_ok() && (*n).is_context_independent != 0 && (*n).memoized == 0 {
        let mut memo = val_zero(T_NODESET);
        /* OOM during the clone: the caller's `out` is still valid, so leave the
         * node unmemoized and surface the error. */
        val_clone(out, &mut memo, err)?;
        /* The AST is read-only at eval time apart from these memo slots. */
        let mut_n = n as *mut Node;
        (*mut_n).memo_value = memo;
        (*mut_n).memoized = 1;
    }
    result
}

/// Evaluate an AST against the context, with the context node as the focus.
///
/// # Safety
/// `ctx` must be a live context and `ast` a live AST built by `mkr_parse`.
pub unsafe fn eval_ast<D: Dom>(
    ctx: *mut Context,
    ast: *const Node,
    out: *mut Val,
    err: ErrSink,
) -> EvalResult {
    let focus = Focus::<D> {
        node: D::from_void(mkr_ctx_node(ctx)),
        pos: 1,
        size: 1,
    };
    eval_node::<D>(ctx, ast, &focus, out, err)
}

extern "C" {
    #[link_name = "fmod"]
    fn libm_fmod(a: f64, b: f64) -> f64;
}
