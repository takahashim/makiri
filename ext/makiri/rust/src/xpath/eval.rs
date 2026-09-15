//! The XPath 1.0 evaluator (mkr_xpath_eval_body.h): axis walks, node tests,
//! predicates, the operator semantics, and the two index fast paths.
//!
//! Generic over `Dom`, so one body compiles per representation - what the C
//! did by `#include`-ing this file twice behind different macros.

/* A failure is `Err(Reported)`: the detail lives in the evaluation's budget, and
 * the proof says it was written. A value comes back as an `OwnedVal`, so one
 * dropped on an error path is cleared. */

use super::abi::*;
use super::attr_pred::{attr_pred_matches, match_attr_pred};
use super::axis::{axis_can_alias, axis_is_implemented, axis_name, is_reverse_axis, walk_axis};
use super::dom::*;
use super::funcs;
use super::msg::Bytes;
use super::nodetest::{node_principal_match, Bindings};
use super::order::nodeset_unique_sorted;
use super::own::{OwnedVal, Set};
use super::step_index::{try_descendant_index, try_descendant_index_nth};
use super::value::*;
use crate::err_setf;
use crate::falloc::{try_vec_with_capacity, Reserve};
use core::ffi::c_void;

/// An evaluation step: the value, or proof its error was written to the
/// evaluation's budget.
type EvalResult<T = ()> = Result<T, Reported>;

/// The per-evaluate memo table: slot `i` holds the value of the subtree whose
/// `Expr::memo` is `Some(i)`, once it has been computed in this evaluate.
///
/// Kept off the AST so a compiled expression is never written during an
/// evaluate, and dropped with the evaluate, so nothing is left to clear.
pub(crate) struct Memo(Vec<Option<OwnedVal>>);

impl Memo {
    fn new(slots: usize, err: ErrSink) -> EvalResult<Memo> {
        let Some(mut table) = try_vec_with_capacity(slots) else {
            return Err(err_setf!(
                err,
                XP_ERR_OOM,
                "out of memory allocating the memo table"
            ));
        };
        table.resize_with(slots, || None);
        Ok(Memo(table))
    }
}

/// One evaluate: everything a walk reads from its context, and everything it
/// changes, which it owns.
///
/// The context is only read here - its registrations, its caps, its document
/// and node - so it is held shared. The state a walk writes (the budget it
/// charges, the string-value cache, the document-order index, the memo table)
/// and the handler answering unknown functions belong to this evaluate alone.
/// A handler that evaluates again on the same context gets an `Evaluation` of
/// its own, so it can neither refill this walk's budget nor take its handler
/// away.
pub struct Evaluation<'e, D: Dom<'e>> {
    pub cx: &'e Context,
    pub doc: D,
    pub budget: Budget,
    pub str_cache: StrCache,
    pub order_index: OrderIndex,
    memo: Memo,
    handler: Option<Handler>,
}

impl<'e, D: Dom<'e>> Evaluation<'e, D> {
    /// One evaluate on `cx`, or None when the context has no document.
    ///
    /// # Safety
    /// `cx`'s document must be this backend's, live and unchanged for `'e`.
    pub(crate) unsafe fn new(cx: &'e Context, handler: Option<Handler>) -> Option<Self> {
        Some(Evaluation {
            cx,
            doc: D::from_document(cx.document())?,
            budget: Budget::with_limits(cx.limits()),
            str_cache: StrCache::new(),
            order_index: OrderIndex::new(),
            memo: Memo(Vec::new()),
            handler,
        })
    }
}

/* ---------- predicates ---------- */

unsafe fn apply_predicates<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    preds: &[Expr],
    inout: &mut Set,
) -> EvalResult {
    let doc = ev.doc;
    for pred in preds {
        let mut kept = Set::new();

        /* Specialise [@name] / [@name='lit'] - position-independent, so applying
         * it per predicate even amid others matches the generic path. */
        if let Some(ap) = match_attr_pred(pred) {
            for i in 0..inout.count() {
                /* Charge per candidate: this replaces a per-node generic
                 * predicate eval, which would tick through eval_node, so the
                 * shortcut stays under the same budget as the path it skips. */
                limit_eval_op(&raw mut ev.budget)?;
                let n = inout.get::<D>(doc, i);
                if attr_pred_matches::<D>(doc, &ap, n) {
                    kept.push::<D>(n, &raw mut ev.budget)?;
                }
            }
            inout.replace(kept.take());
            continue;
        }

        let size = inout.count();
        for i in 0..size {
            let n = inout.get::<D>(doc, i);
            let pf = Focus {
                node: Some(n),
                pos: i + 1,
                size,
            };
            let v = eval_node::<D>(ev, pred, &pf)?;
            /* A bare number predicate means position() = that number. */
            let keep = match v.get() {
                ValRef::Number(d) => d == (i + 1) as f64,
                _ => val_to_boolean(&*v),
            };
            if keep {
                kept.push::<D>(n, &raw mut ev.budget)?;
            }
        }
        inout.replace(kept.take());
    }
    Ok(())
}

/* ---------- steps ---------- */

/// The URI a name test's prefix is bound to, or the RUNTIME error a step reports
/// for an unknown one.
///
/// The borrow lives in the context's registry, and the glue refuses
/// register_namespace (and register_variable, node=) while an evaluate is in
/// progress on the context - which is exactly when a predicate handler could
/// re-enter.
fn resolve_test_prefix<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    test: &NodeTest,
) -> EvalResult<Option<&'e [u8]>> {
    let Some(prefix) = test.prefix.as_deref() else {
        return Ok(None);
    };
    let cx: &'e Context = ev.cx;
    match cx.lookup_ns(prefix) {
        Some(u) => Ok(Some(u)),
        None => Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_RUNTIME,
            "unknown namespace prefix '{}' in name test",
            Bytes(prefix)
        )),
    }
}

unsafe fn eval_step<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    step: &Step,
    context_set: &Set,
    out: &mut Set,
) -> EvalResult {
    let doc = ev.doc;
    let axis = step.axis;
    if !axis_is_implemented(axis) {
        return Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_NOT_IMPLEMENTED,
            "native engine: axis '{}' not implemented yet",
            axis_name(axis)
        ));
    }
    let test = &step.test;
    let budget: *mut Budget = &raw mut ev.budget;

    /* Resolve the namespace prefix once up front (covering `prefix:local` and
     * `prefix:*`): a uniform RUNTIME error rather than a silently empty match,
     * and every per-node match then reuses the URI instead of re-resolving. */
    let pre = resolve_test_prefix(ev, test)?;

    let b = Bindings::<D>::new(ev.cx, doc, pre);

    /* A post-pass (sort to document order, then optional adjacent dedup) is
     * needed when the axis emits in reverse order per context, when it aliases
     * across contexts, or when several contexts produce results that interleave.
     * child is the canonical third case: html's children are head and body, and
     * head's children include title, so naive concatenation gives
     * [head, body, title] where document order is [head, title, body]. Only self
     * and attribute stay in order under concatenation. */
    let need_post_pass = is_reverse_axis(axis)
        || (axis_can_alias(axis) && context_set.count() > 1)
        || (context_set.count() > 1 && axis != Axis::SelfAxis && axis != Axis::Attribute);

    let mut result = Set::new();

    let preds = step.predicates.as_slice();
    if preds.is_empty() {
        if !try_descendant_index::<D>(doc, step, context_set, &mut result, &b, budget)? {
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
                    if let Err(e) = limit_eval_op(budget) {
                        failure = Some(e);
                        return true;
                    }
                    if node_principal_match::<D>(doc, test, n, axis, &b) {
                        if let Err(e) = result.push::<D>(n, budget) {
                            failure = Some(e);
                            return true;
                        }
                    }
                    false
                };
                walk_axis::<D, _>(doc, axis, context_set.get::<D>(doc, ci), &mut visit);
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
                    if let Err(e) = limit_eval_op(budget) {
                        failure = Some(e);
                        return true;
                    }
                    if node_principal_match::<D>(doc, test, n, axis, &b) {
                        if let Err(e) = frag.push::<D>(n, budget) {
                            failure = Some(e);
                            return true;
                        }
                    }
                    false
                };
                walk_axis::<D, _>(doc, axis, context_set.get::<D>(doc, ci), &mut visit);
            }
            if let Some(e) = failure {
                return Err(e);
            }

            /* Predicates apply per context with axis-natural position numbering
             * (§2.4). For a reverse axis the fragment is in reverse-document
             * order, so [1] is the closest to the context - the intended
             * meaning. */
            apply_predicates::<D>(ev, preds, &mut fragment)?;
            for i in 0..fragment.count() {
                result.push::<D>(fragment.get::<D>(doc, i), &raw mut ev.budget)?;
            }
        }
    }

    if need_post_pass && result.count() > 1 {
        nodeset_unique_sorted::<D>(ev, result.as_mut());
    }
    out.replace(result.take());
    Ok(())
}

unsafe fn eval_steps<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    steps: &[Step],
    seed: &mut Set,
) -> EvalResult<OwnedVal> {
    let mut current = Set::adopt(seed.take());
    let mut rest = steps;

    if let [s0, s1, ..] = steps {
        let mut nth = Set::new();
        if try_descendant_index_nth::<D>(ev, s0, s1, &current, &mut nth)? {
            current = Set::adopt(nth.take());
            rest = &steps[2..];
        }
    }
    for step in rest {
        let mut next = Set::new();
        eval_step::<D>(ev, step, &current, &mut next)?;
        current = Set::adopt(next.take());
    }
    Ok(Val::nodeset(current.take()).into())
}

/* ---------- comparisons ---------- */

/// §3.4 equality. A node-set on either side means "true iff SOME node satisfies
/// it"; all node string-values go through the per-evaluate cache, so an M-by-N
/// comparison costs O(M+N) string builds.
unsafe fn compare_eq<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    l: &Val,
    r: &Val,
    op: Op,
) -> EvalResult<bool> {
    let doc = ev.doc;
    let want_eq = op == Op::Eq;

    let (set, sc) = match (l.as_nodeset(), r.as_nodeset()) {
        (Some(ls), Some(rs)) => {
            /* The pair scan itself is M*N even though the string builds are
             * O(M+N), so charge each pair: otherwise an all-pairs node-set
             * equality drives up to ~1e14 comparisons as a handful of ops. */
            for i in 0..ls.count {
                let a = cached_node_text::<D>(ev, nodeset_at::<D>(doc, ls, i))?;
                for j in 0..rs.count {
                    limit_eval_op(&raw mut ev.budget)?;
                    let b = cached_node_text::<D>(ev, nodeset_at::<D>(doc, rs, j))?;
                    if (ev.str_cache.text(a) == ev.str_cache.text(b)) == want_eq {
                        return Ok(true);
                    }
                }
            }
            return Ok(false);
        }
        (Some(set), None) => (set, r),
        (None, Some(set)) => (set, l),
        (None, None) => {
            let eq = match (l.get(), r.get()) {
                (ValRef::Boolean(_), _) | (_, ValRef::Boolean(_)) => {
                    val_to_boolean(l) == val_to_boolean(r)
                }
                /* Both operands are non-node-sets here, so the unchecked
                 * coercion is the right entry - it cannot allocate. */
                (ValRef::Number(_), _) | (_, ValRef::Number(_)) => {
                    val_to_number_unchecked::<D>(doc, l) == val_to_number_unchecked::<D>(doc, r)
                }
                _ => {
                    let ls = val_to_owned_text_or_fail::<D>(doc, l, &raw mut ev.budget)?;
                    let rs = val_to_owned_text_or_fail::<D>(doc, r, &raw mut ev.budget)?;
                    ls.as_slice() == rs.as_slice()
                }
            };
            return Ok(if want_eq { eq } else { !eq });
        }
    };
    match sc.get() {
        ValRef::Number(target) => {
            for i in 0..set.count {
                limit_eval_op(&raw mut ev.budget)?;
                let s = cached_node_number::<D>(ev, nodeset_at::<D>(doc, set, i))?;
                if (s == target) == want_eq {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ValRef::Boolean(b) => {
            let eq = (set.count > 0) == b;
            Ok(if want_eq { eq } else { !eq })
        }
        _ => {
            let target = val_to_owned_text_or_fail::<D>(doc, sc, &raw mut ev.budget)?;
            let want = target.as_slice();
            for i in 0..set.count {
                limit_eval_op(&raw mut ev.budget)?;
                let s = cached_node_text::<D>(ev, nodeset_at::<D>(doc, set, i))?;
                if (ev.str_cache.text(s) == want) == want_eq {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn rel_hit(op: Op, a: f64, b: f64) -> bool {
    match op {
        Op::Lt => a < b,
        Op::Le => a <= b,
        Op::Gt => a > b,
        Op::Ge => a >= b,
        _ => false,
    }
}

/// §3.4 relational. A node-set on either side is true iff SOME pair satisfies
/// the relation on their numeric string-values - every pair, not just the first
/// node of each side.
unsafe fn compare_rel<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    l: &Val,
    r: &Val,
    op: Op,
) -> EvalResult<bool> {
    let doc = ev.doc;

    /* `swap` records that the node-set is the right operand, so each pair is
     * compared in source order. */
    let (set, sc, swap) = match (l.as_nodeset(), r.as_nodeset()) {
        (Some(ls), Some(rs)) => {
            for i in 0..ls.count {
                let a = cached_node_number::<D>(ev, nodeset_at::<D>(doc, ls, i))?;
                for j in 0..rs.count {
                    limit_eval_op(&raw mut ev.budget)?;
                    let b = cached_node_number::<D>(ev, nodeset_at::<D>(doc, rs, j))?;
                    if rel_hit(op, a, b) {
                        return Ok(true);
                    }
                }
            }
            return Ok(false);
        }
        (Some(set), None) => (set, r, false),
        (None, Some(set)) => (set, l, true),
        (None, None) => {
            let a = val_to_number_or_fail::<D>(doc, l, &raw mut ev.budget)?;
            let b = val_to_number_or_fail::<D>(doc, r, &raw mut ev.budget)?;
            return Ok(rel_hit(op, a, b));
        }
    };
    let scn = val_to_number_or_fail::<D>(doc, sc, &raw mut ev.budget)?;
    for i in 0..set.count {
        limit_eval_op(&raw mut ev.budget)?;
        let nv = cached_node_number::<D>(ev, nodeset_at::<D>(doc, set, i))?;
        let (a, b) = if swap { (scn, nv) } else { (nv, scn) };
        if rel_hit(op, a, b) {
            return Ok(true);
        }
    }
    Ok(false)
}

/* ---------- union ---------- */

unsafe fn union_nodeset<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    l: &Val,
    r: &Val,
) -> EvalResult<OwnedVal> {
    let (Some(ls), Some(rs)) = (l.as_nodeset(), r.as_nodeset()) else {
        return Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_TYPE,
            "operands of '|' must be node-sets"
        ));
    };
    /* Push both sides without deduplicating per insert - that was quadratic -
     * then sort once and collapse adjacent duplicates. */
    let doc = ev.doc;
    let mut merged = Set::new();
    for set in [ls, rs] {
        for i in 0..set.count {
            merged.push::<D>(nodeset_at::<D>(doc, set, i), &raw mut ev.budget)?;
        }
    }
    /* §3.3: the result of '|' is a node-set in document order, which the
     * downstream string() / number() / positional predicates assume. */
    nodeset_unique_sorted::<D>(ev, merged.as_mut());
    Ok(Val::nodeset(merged.take()).into())
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
fn first_recognise(root: &Expr) -> Option<&Step> {
    let ExprKind::Path(path) = &root.kind else {
        return None;
    };
    let nt = match path.steps.as_slice() {
        [s] if s.axis == Axis::Descendant => s,
        [s0, s1]
            if s0.axis == Axis::DescendantOrSelf
                && s0.test.kind == TestKind::Node
                && s0.predicates.is_empty()
                && s1.axis == Axis::Child =>
        {
            s1
        }
        _ => return None,
    };
    /* A prefixed name test is allowed - the caller reproduces the step driver's
     * "unknown prefix is a RUNTIME error" first, and the name match resolves the
     * prefix exactly as the full evaluator does. A prefixed ATTRIBUTE predicate
     * still falls back: match_attr_step requires an unprefixed @name. */
    for p in &nt.predicates {
        match_attr_pred(p)?;
    }
    Some(nt)
}

/// Does `n` satisfy every already-recognised attribute predicate of `step`?
fn first_node_ok<'e, D: Dom<'e>>(doc: D, step: &Step, n: D::Node) -> bool {
    for p in &step.predicates {
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
/// `cx`'s document must be this backend's.
#[allow(clippy::result_large_err)]
pub unsafe fn try_first_match<'e, D: Dom<'e>>(
    cx: &'e Context,
    ast: &Ast,
) -> Result<Option<*mut c_void>, Error> {
    let Some(mut ev) = Evaluation::<D>::new(cx, None) else {
        return Ok(None); /* no document: the full evaluator reports it */
    };
    match first_match_walk::<D>(&mut ev, ast) {
        Ok(found) => Ok(found.map(|n| n.map_or(core::ptr::null_mut(), D::token))),
        Err(_) => Err(ev.budget.take_error()),
    }
}

unsafe fn first_match_walk<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    ast: &Ast,
) -> EvalResult<Option<Option<D::Node>>> {
    let doc = ev.doc;
    let root = ast.root();
    let step = match first_recognise(root) {
        Some(s) => s,
        None => return Ok(None),
    };
    let test = &step.test;

    /* Reproduce the step driver's prefix validation, so the fast path stays
     * identical to the full evaluator down to the errors - and keep what it
     * resolved, so the walk below does not look the prefix up again per node. */
    let pre = resolve_test_prefix(ev, test)?;

    let absolute = matches!(&root.kind, ExprKind::Path(p) if p.absolute);
    let start = if absolute {
        Some(doc.document_node())
    } else {
        let p = ev.cx.context_node();
        (!p.is_null()).then(|| doc.node(p))
    };
    let Some(start) = start else {
        return Ok(Some(None)); /* recognised; no context means no match */
    };

    let b = Bindings::<D>::new(ev.cx, doc, pre);
    let mut cur = doc.first_child(start);
    while let Some(n) = cur {
        limit_eval_op(&raw mut ev.budget)?;
        if node_principal_match::<D>(doc, test, n, step.axis, &b)
            && first_node_ok::<D>(doc, step, n)
        {
            return Ok(Some(Some(n)));
        }
        if let Some(c) = doc.first_child(n) {
            cur = Some(c);
            continue;
        }
        /* Past `n`'s subtree, without leaving `start`'s. */
        let mut m = n;
        cur = loop {
            if m == start {
                break None;
            }
            if let Some(s) = doc.next(m) {
                break Some(s);
            }
            match doc.parent(m) {
                Some(p) => m = p,
                None => break None,
            }
        };
    }
    Ok(Some(None))
}

/* ---------- the expression evaluator ---------- */

unsafe fn eval_path<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    p: &Path,
    self_node: Option<D::Node>,
) -> EvalResult<OwnedVal> {
    let mut seed = Set::new();
    let start = if p.absolute {
        Some(ev.doc.document_node())
    } else {
        self_node
    };
    if let Some(n) = start {
        seed.push::<D>(n, &raw mut ev.budget)?;
    }
    eval_steps::<D>(ev, &p.steps, &mut seed)
}

unsafe fn eval_filter<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    expr: &Expr,
    predicates: &[Expr],
    steps: &[Step],
    focus: &Focus<'e, D>,
) -> EvalResult<OwnedVal> {
    let mut primary = eval_node::<D>(ev, expr, focus)?;
    if !predicates.is_empty() {
        let Some(ns) = primary.as_nodeset_mut() else {
            return Err(err_setf!(
                ev.budget.sink(),
                XP_ERR_TYPE,
                "predicate applied to non-node-set"
            ));
        };
        /* Filtered in a guard, so a failing predicate frees the set. */
        let mut set = Set::adopt(core::mem::replace(ns, NodeSet::EMPTY));
        apply_predicates::<D>(ev, predicates, &mut set)?;
        *ns = set.take();
    }
    if !steps.is_empty() {
        let Some(ns) = primary.as_nodeset_mut() else {
            return Err(err_setf!(
                ev.budget.sink(),
                XP_ERR_TYPE,
                "path applied to non-node-set"
            ));
        };
        let mut seed = Set::adopt(core::mem::replace(ns, NodeSet::EMPTY));
        return eval_steps::<D>(ev, steps, &mut seed);
    }
    Ok(primary)
}

unsafe fn eval_fncall<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    prefix: Option<&[u8]>,
    name: &[u8],
    args: &[Expr],
    focus: &Focus<'e, D>,
) -> EvalResult<OwnedVal> {
    let cx = ev.cx;
    let ns_uri: Option<&[u8]> = match prefix {
        None => None,
        Some(prefix) => match cx.lookup_ns(prefix) {
            Some(u) => Some(u),
            None => {
                return Err(err_setf!(
                    ev.budget.sink(),
                    XP_ERR_RUNTIME,
                    "unknown namespace prefix '{}'",
                    Bytes(prefix)
                ));
            }
        },
    };
    let builtin = funcs::lookup::<D>(ns_uri, name);

    /* The arguments are evaluated once and reused by either path. They are owned
     * here, so every way out - an argument failing part-way included - clears
     * them when `vals` drops. */
    let mut vals: Vec<OwnedVal> = Vec::new();
    if !args.is_empty() {
        if vals.mkr_reserve_exact(args.len()).is_err() {
            return Err(err_setf!(
                ev.budget.sink(),
                XP_ERR_OOM,
                "out of memory allocating function arguments"
            ));
        }
        for a in args {
            vals.push(eval_node::<D>(ev, a, focus)?);
        }
    }

    if let Some(f) = builtin {
        return f(ev, focus, OwnedVal::as_vals(&vals));
    }

    /* No built-in. Delegate to this evaluate's handler, when it has one. */
    let answer = match ev.handler {
        Some(handler) => {
            let site = ResolverCall {
                node: focus.node.map_or(core::ptr::null_mut(), D::token),
                pos: focus.pos,
                size: focus.size,
                ns_uri,
                local: name,
                args: OwnedVal::as_vals(&vals),
            };
            (handler.resolve)(handler.data, &mut ev.budget, &site)?
        }
        None => None,
    };
    answer.ok_or_else(|| {
        err_setf!(
            ev.budget.sink(),
            XP_ERR_RUNTIME,
            "unknown function {}{}{}",
            Bytes(prefix.unwrap_or(&[])),
            if prefix.is_none() { "" } else { ":" },
            Bytes(name)
        )
    })
}

unsafe fn eval_binop<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    op: Op,
    lhs: &Expr,
    rhs: &Expr,
    focus: &Focus<'e, D>,
) -> EvalResult<OwnedVal> {
    let doc = ev.doc;

    /* and / or short-circuit. */
    if op == Op::Or || op == Op::And {
        let l = eval_node::<D>(ev, lhs, focus)?;
        let lb = val_to_boolean(&*l);
        if (op == Op::Or && lb) || (op == Op::And && !lb) {
            return Ok(Val::boolean(lb).into());
        }
        let r = eval_node::<D>(ev, rhs, focus)?;
        return Ok(Val::boolean(val_to_boolean(&*r)).into());
    }

    let l = eval_node::<D>(ev, lhs, focus)?;
    let r = eval_node::<D>(ev, rhs, focus)?;
    let (l, r): (&Val, &Val) = (&l, &r);

    match op {
        Op::Eq | Op::Ne => Ok(Val::boolean(compare_eq::<D>(ev, l, r, op)?).into()),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            Ok(Val::boolean(compare_rel::<D>(ev, l, r, op)?).into())
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod => {
            let a = val_to_number_or_fail::<D>(doc, l, &raw mut ev.budget)?;
            let c = val_to_number_or_fail::<D>(doc, r, &raw mut ev.budget)?;
            Ok(Val::number(match op {
                Op::Add => a + c,
                Op::Sub => a - c,
                Op::Mul => a * c,
                Op::Div => a / c,
                _ => libm_fmod(a, c),
            })
            .into())
        }
        /* union_nodeset reports its own typed error; do not overwrite it. */
        Op::Union => union_nodeset::<D>(ev, l, r),
        _ => Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_INTERNAL,
            "unexpected binop"
        )),
    }
}

/// Unary minus: the operand as a number, negated.
unsafe fn eval_negate<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    x: &Expr,
    focus: &Focus<'e, D>,
) -> EvalResult<OwnedVal> {
    let doc = ev.doc;
    let v = eval_node::<D>(ev, x, focus)?;
    let d = val_to_number_or_fail::<D>(doc, &*v, &raw mut ev.budget)?;
    Ok(Val::number(-d).into())
}

/// A string result copied from `bytes`.
unsafe fn string_value(bytes: &[u8], err: ErrSink, what: &core::ffi::CStr) -> EvalResult<OwnedVal> {
    let mut text = owned_copy(bytes, err, what)?;
    Ok(Val::string(text.take()).into())
}

/// The evaluator's only recursive function, and therefore the whole of "AST
/// recursion is bounded": one op and one recursion level are charged on entry
/// and the level is released at the single exit. Keeping it single-exit is what
/// makes that balance locally checkable.
unsafe fn eval_node<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    e: &Expr,
    focus: &Focus<'e, D>,
) -> EvalResult<OwnedVal> {
    ev.budget.charge_op()?;
    /* A refused entry is not counted, so returning here needs no release. */
    ev.budget.enter_recursion()?;
    let result = eval_node_inner::<D>(ev, e, focus);
    ev.budget.leave_recursion();
    result
}

unsafe fn eval_node_inner<'e, D: Dom<'e>>(
    ev: &mut Evaluation<'e, D>,
    e: &Expr,
    focus: &Focus<'e, D>,
) -> EvalResult<OwnedVal> {
    /* Hoisting: a context-independent subtree already computed in this evaluate
     * comes back as a clone, which keeps ownership clean - clearing either copy
     * is safe. */
    if let Some(slot) = e.memo {
        if let Some(v) = &ev.memo.0[slot as usize] {
            return val_clone(v, ev.budget.sink());
        }
    }

    let cx = ev.cx;
    let value = match &e.kind {
        ExprKind::LiteralStr(t) => {
            string_value(t, ev.budget.sink(), c"out of memory copying literal")
        }
        ExprKind::LiteralNum(d) => Ok(Val::number(*d).into()),
        ExprKind::VarRef { prefix, name } => match cx.variable_text(prefix.as_deref(), name) {
            Some(bytes) => string_value(
                bytes,
                ev.budget.sink(),
                c"out of memory copying variable value",
            ),
            None => Err(err_setf!(
                ev.budget.sink(),
                XP_ERR_RUNTIME,
                "undefined variable ${}{}{}",
                Bytes(prefix.as_deref().unwrap_or(&[])),
                if prefix.is_none() { "" } else { ":" },
                Bytes(name)
            )),
        },
        ExprKind::FnCall { prefix, name, args } => {
            eval_fncall::<D>(ev, prefix.as_deref(), name, args, focus)
        }
        ExprKind::Negate(x) => eval_negate::<D>(ev, x, focus),
        ExprKind::BinOp { op, lhs, rhs } => eval_binop::<D>(ev, *op, lhs, rhs, focus),
        ExprKind::Path(p) => eval_path::<D>(ev, p, focus.node),
        ExprKind::Filter {
            expr,
            predicates,
            steps,
        } => eval_filter::<D>(ev, expr, predicates, steps, focus),
    }?;

    /* Remember a context-independent subtree on success. The clone keeps the
     * caller's value independent of the remembered one, which matters because
     * the caller is free to consume theirs. */
    if let Some(slot) = e.memo {
        if ev.memo.0[slot as usize].is_none() {
            /* OOM during the clone drops `value` with the error. */
            let memo = val_clone(&value, ev.budget.sink())?;
            ev.memo.0[slot as usize] = Some(memo);
        }
    }
    Ok(value)
}

/// Evaluate an AST against the context, with the context node as the focus, on
/// a fresh evaluation that `handler` answers unknown functions for.
///
/// # Safety
/// `cx`'s document must be this backend's, and `ast` parsed for it.
#[allow(clippy::result_large_err)]
pub unsafe fn eval_ast<'e, D: Dom<'e>>(
    cx: &'e Context,
    ast: &Ast,
    handler: Option<Handler>,
) -> Result<OwnedVal, Error> {
    let Some(mut ev) = Evaluation::<D>::new(cx, handler) else {
        let mut budget = Budget::with_limits(cx.limits());
        let _ = err_setf!(budget.sink(), XP_ERR_RUNTIME, "evaluate with no document");
        return Err(budget.take_error());
    };
    let result = match Memo::new(ast.memo_slots(), ev.budget.sink()) {
        Ok(memo) => {
            ev.memo = memo;
            let p = cx.context_node();
            let focus = Focus {
                node: (!p.is_null()).then(|| ev.doc.node(p)),
                pos: 1,
                size: 1,
            };
            eval_node::<D>(&mut ev, ast.root(), &focus)
        }
        Err(e) => Err(e),
    };
    result.map_err(|_| ev.budget.take_error())
}

extern "C" {
    #[link_name = "fmod"]
    fn libm_fmod(a: f64, b: f64) -> f64;
}
