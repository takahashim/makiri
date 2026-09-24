//! The XPath 1.0 evaluator: axis walks, node tests, predicates, the operator
//! semantics, and the two index fast paths.
//!
//! Generic over `Dom`, so one body compiles per representation.

#![forbid(unsafe_code)]

/* A failure is `Err(Reported)`: the detail lives in the evaluation's budget, and
 * the proof says it was written. A value comes back as a `Val`, so one
 * dropped on an error path is cleared. */

use super::abi::*;
use super::attr_pred::{attr_pred_matches, match_attr_pred};
use super::axis::{
    axis_can_alias, axis_is_implemented, axis_name, is_reverse_axis, walk_axis, walk_descendants,
};
use super::dom::*;
use super::funcs;
use super::msg::Bytes;
use super::nodetest::CompiledTest;
use super::order::nodeset_unique_sorted;
use super::step_index::{try_descendant_index, try_descendant_index_nth};
use super::value::*;
use crate::err_setf;
use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::token::Token;
use core::ops::ControlFlow;

/// An evaluation step: the value, or proof its error was written to the
/// evaluation's budget.
type EvalResult<T = ()> = Result<T, Reported>;

/// The per-evaluate memo table: slot `i` holds the value of the subtree whose
/// `Expr::memo` is `Some(i)`, once it has been computed in this evaluate.
///
/// Kept off the AST so a compiled expression is never written during an
/// evaluate, and dropped with the evaluate, so nothing is left to clear.
pub(crate) struct Memo<N>(Vec<Option<Val<N>>>);

impl<N> Memo<N> {
    fn new(slots: usize, err: ErrSink) -> EvalResult<Memo<N>> {
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
pub struct Evaluation<'e, 'd, D: Dom<'d>> {
    pub cx: &'e Context<'d, D>,
    pub names: &'e Names,
    pub doc: D,
    pub budget: Budget,
    pub str_cache: StrCache,
    pub order_index: OrderIndex,
    /// The CSS lowering's sibling positions, one parent at a time.
    pub(crate) sibling_positions: super::funcs::SiblingPositions<D::Node>,
    memo: Memo<D::Node>,
    handler: Option<&'e dyn Resolver>,
}

impl<'e, 'd, D: Dom<'d>> Evaluation<'e, 'd, D> {
    /// One evaluate of `doc` under `cx`, whose registrations are `names`.
    fn new(
        cx: &'e Context<'d, D>,
        names: &'e Names,
        doc: D,
        handler: Option<&'e dyn Resolver>,
    ) -> Self {
        Evaluation {
            cx,
            names,
            doc,
            budget: Budget::with_limits(cx.limits()),
            str_cache: StrCache::new(),
            order_index: OrderIndex::new(),
            sibling_positions: super::funcs::SiblingPositions::new(),
            memo: Memo(Vec::new()),
            handler,
        }
    }
}

/* ---------- predicates ---------- */

fn apply_predicates<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    preds: &[Expr],
    inout: &mut NodeSet<D::Node>,
) -> EvalResult {
    let doc = ev.doc;
    let lax = ev.cx.lax();
    for pred in preds {
        let mut kept = NodeSet::new();

        /* Specialise [@name] / [@name='lit'] - position-independent, so applying
         * it per predicate even amid others matches the generic path. */
        if let Some(ap) = match_attr_pred(pred) {
            for i in 0..inout.len() {
                /* Charge per candidate: this replaces a per-node generic
                 * predicate eval, which would tick through eval_node, so the
                 * shortcut stays under the same budget as the path it skips. */
                ev.budget.charge_op()?;
                let n = inout.get(i);
                if attr_pred_matches::<D>(doc, &ap, n, lax) {
                    kept.push(n, &mut ev.budget)?;
                }
            }
            *inout = kept;
            continue;
        }

        let size = inout.len();
        for i in 0..size {
            let n = inout.get(i);
            let pf = Focus {
                node: Some(n),
                pos: i + 1,
                size,
            };
            let v = eval_node::<D>(ev, pred, &pf)?;
            /* A bare number predicate means position() = that number. */
            let keep = match v.get() {
                ValRef::Number(d) => d == (i + 1) as f64,
                _ => val_to_boolean(&v),
            };
            if keep {
                kept.push(n, &mut ev.budget)?;
            }
        }
        *inout = kept;
    }
    Ok(())
}

/* ---------- steps ---------- */

/// Every node of `ct`'s axis from `context` that passes the test, appended to
/// `out` - the one walk the step driver makes, predicate path or not.
///
/// Every visited node is charged to the budget. The axis walk is the dominant
/// work of a step, and a low-selectivity walk name-tests many nodes while
/// pushing few - so without this the node-set cap, which bounds only what is
/// pushed, leaves the walk itself bounded by document size, defeating
/// max_eval_ops on a descendant walk that matches nothing.
fn collect_axis<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    ct: &CompiledTest<'_>,
    context: D::Node,
    out: &mut NodeSet<D::Node>,
) -> EvalResult {
    let (doc, budget) = (ev.doc, &mut ev.budget);
    let flow = walk_axis::<D, _, _>(doc, ct.axis(), context, &mut |n| {
        let visited = budget.charge_op().and_then(|()| {
            if ct.matches(doc, n) {
                out.push(n, budget)
            } else {
                Ok(())
            }
        });
        match visited {
            Ok(()) => ControlFlow::Continue(()),
            Err(e) => ControlFlow::Break(e),
        }
    });
    match flow {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(e) => Err(e),
    }
}

fn eval_step<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    step: &Step,
    context_set: &NodeSet<D::Node>,
    out: &mut NodeSet<D::Node>,
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

    /* Resolve the namespace prefix once up front (covering `prefix:local` and
     * `prefix:*`), and every per-node match then reuses it. */
    let names: &'e Names = ev.names;
    let ct = CompiledTest::new(&step.test, axis, names, ev.cx.lax(), ev.budget.sink())?;

    /* A post-pass (sort to document order, then optional adjacent dedup) is
     * needed when the axis emits in reverse order per context, when it aliases
     * across contexts, or when several contexts produce results that interleave.
     * child is the canonical third case: html's children are head and body, and
     * head's children include title, so naive concatenation gives
     * [head, body, title] where document order is [head, title, body]. Only self
     * and attribute stay in order under concatenation. */
    let need_post_pass = is_reverse_axis(axis)
        || (axis_can_alias(axis) && context_set.len() > 1)
        || (context_set.len() > 1 && axis != Axis::SelfAxis && axis != Axis::Attribute);

    let mut result = NodeSet::new();

    let preds = step.predicates.as_slice();
    if preds.is_empty() {
        if !try_descendant_index::<D>(doc, &ct, context_set, &mut result, &mut ev.budget)? {
            /* No-predicate walk: every context goes straight into the result
             * buffer regardless of the post-pass, saving the per-context
             * fragment the predicate path needs. */
            for ci in 0..context_set.len() {
                collect_axis(ev, &ct, context_set.get(ci), &mut result)?;
            }
        }
    } else {
        /* Predicate path: position() and last() are per-context, so each
         * context's fragment has to be materialised before filtering. One
         * fragment buffer is reused across iterations, so its storage grows to
         * the largest single-context cardinality once rather than per iteration. */
        let mut fragment = NodeSet::new();
        for ci in 0..context_set.len() {
            fragment.clear();
            collect_axis(ev, &ct, context_set.get(ci), &mut fragment)?;

            /* Predicates apply per context with axis-natural position numbering
             * (§2.4). For a reverse axis the fragment is in reverse-document
             * order, so [1] is the closest to the context - the intended
             * meaning. */
            apply_predicates::<D>(ev, preds, &mut fragment)?;
            for i in 0..fragment.len() {
                result.push(fragment.get(i), &mut ev.budget)?;
            }
        }
    }

    if need_post_pass && result.len() > 1 {
        if context_set.len() == 1 && is_reverse_axis(axis) {
            /* One context on a reverse axis emits exactly reverse document
             * order, each node once (a predicate only removes some), so the
             * sort is a reversal - O(n). Sorting it cost a full merge sort of
             * hash lookups per step, uncharged: `count(preceding-sibling::i)`
             * per candidate over 4000 siblings took 3.9 s, following-sibling
             * 0.2 s. */
            result.as_mut_slice().reverse();
        } else {
            nodeset_unique_sorted::<D>(ev, &mut result);
        }
    }
    *out = result;
    Ok(())
}

fn eval_steps<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    steps: &[Step],
    seed: &mut NodeSet<D::Node>,
) -> EvalResult<Val<D::Node>> {
    let mut current = core::mem::take(seed);
    let mut rest = steps;

    if let [s0, s1, ..] = steps {
        let mut nth = NodeSet::new();
        if try_descendant_index_nth::<D>(ev, s0, s1, &current, &mut nth)? {
            current = nth;
            rest = &steps[2..];
        }
    }
    for step in rest {
        let mut next = NodeSet::new();
        eval_step::<D>(ev, step, &current, &mut next)?;
        current = next;
    }
    Ok(Val::nodeset(current))
}

/* ---------- comparisons ---------- */

/// §3.4 equality. A node-set on either side means "true iff SOME node satisfies
/// it"; all node string-values go through the per-evaluate cache, so an M-by-N
/// comparison costs O(M+N) string builds.
fn compare_eq<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    l: &Val<D::Node>,
    r: &Val<D::Node>,
    op: Op,
) -> EvalResult<bool> {
    let doc = ev.doc;
    let want_eq = op == Op::Eq;

    let (set, sc) = match (l.as_nodeset(), r.as_nodeset()) {
        (Some(ls), Some(rs)) => {
            /* The pair scan itself is M*N even though the string builds are
             * O(M+N), so charge each pair: otherwise an all-pairs node-set
             * equality drives up to ~1e14 comparisons as a handful of ops. */
            for i in 0..ls.len() {
                let a = cached_node_text::<D>(ev, ls.get(i))?;
                for j in 0..rs.len() {
                    ev.budget.charge_op()?;
                    let b = cached_node_text::<D>(ev, rs.get(j))?;
                    if (a.bytes(&ev.str_cache) == b.bytes(&ev.str_cache)) == want_eq {
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
                /* Neither operand is a node-set here, so both coerce without
                 * allocating. */
                (ValRef::Number(_), _) | (_, ValRef::Number(_)) => {
                    scalar_to_number(l) == scalar_to_number(r)
                }
                _ => {
                    let ls = val_to_owned_text_or_fail::<D>(doc, l, &mut ev.budget)?;
                    let rs = val_to_owned_text_or_fail::<D>(doc, r, &mut ev.budget)?;
                    ls.as_slice() == rs.as_slice()
                }
            };
            return Ok(if want_eq { eq } else { !eq });
        }
    };
    match sc.get() {
        ValRef::Number(target) => {
            for i in 0..set.len() {
                ev.budget.charge_op()?;
                let s = cached_node_number::<D>(ev, set.get(i))?;
                if (s == target) == want_eq {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ValRef::Boolean(b) => {
            let eq = (!set.is_empty()) == b;
            Ok(if want_eq { eq } else { !eq })
        }
        _ => {
            let target = val_to_owned_text_or_fail::<D>(doc, sc, &mut ev.budget)?;
            let want = target.as_slice();
            for i in 0..set.len() {
                ev.budget.charge_op()?;
                let s = cached_node_text::<D>(ev, set.get(i))?;
                if (s.bytes(&ev.str_cache) == want) == want_eq {
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
fn compare_rel<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    l: &Val<D::Node>,
    r: &Val<D::Node>,
    op: Op,
) -> EvalResult<bool> {
    let doc = ev.doc;

    /* `swap` records that the node-set is the right operand, so each pair is
     * compared in source order. */
    let (set, sc, swap) = match (l.as_nodeset(), r.as_nodeset()) {
        (Some(ls), Some(rs)) => {
            for i in 0..ls.len() {
                let a = cached_node_number::<D>(ev, ls.get(i))?;
                for j in 0..rs.len() {
                    ev.budget.charge_op()?;
                    let b = cached_node_number::<D>(ev, rs.get(j))?;
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
            let a = val_to_number_or_fail::<D>(doc, l, &mut ev.budget)?;
            let b = val_to_number_or_fail::<D>(doc, r, &mut ev.budget)?;
            return Ok(rel_hit(op, a, b));
        }
    };
    /* §3.4, as `compare_eq` has it: against a boolean the node-set is its own
     * boolean(), not each node's number. `//p > true()` over <p>5</p> was true
     * (5 > 1) where the spec says false (1 > 1), and `//q < true()` over no q
     * false where it says true (0 < 1). */
    if let ValRef::Boolean(b) = sc.get() {
        let setn = if set.is_empty() { 0.0 } else { 1.0 };
        let bn = if b { 1.0 } else { 0.0 };
        let (a, c) = if swap { (bn, setn) } else { (setn, bn) };
        return Ok(rel_hit(op, a, c));
    }
    let scn = val_to_number_or_fail::<D>(doc, sc, &mut ev.budget)?;
    for i in 0..set.len() {
        ev.budget.charge_op()?;
        let nv = cached_node_number::<D>(ev, set.get(i))?;
        let (a, b) = if swap { (scn, nv) } else { (nv, scn) };
        if rel_hit(op, a, b) {
            return Ok(true);
        }
    }
    Ok(false)
}

/* ---------- union ---------- */

fn union_nodeset<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    l: &Val<D::Node>,
    r: &Val<D::Node>,
) -> EvalResult<Val<D::Node>> {
    let (Some(ls), Some(rs)) = (l.as_nodeset(), r.as_nodeset()) else {
        return Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_TYPE,
            "operands of '|' must be node-sets"
        ));
    };
    /* Push both sides without deduplicating per insert - that was quadratic -
     * then sort once and collapse adjacent duplicates. */
    let mut merged = NodeSet::new();
    for set in [ls, rs] {
        for i in 0..set.len() {
            merged.push(set.get(i), &mut ev.budget)?;
        }
    }
    /* §3.3: the result of '|' is a node-set in document order, which the
     * downstream string() / number() / positional predicates assume. */
    nodeset_unique_sorted::<D>(ev, &mut merged);
    Ok(Val::nodeset(merged))
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
fn first_node_ok<'e, 'd, D: Dom<'d>>(doc: D, step: &Step, n: D::Node, lax: bool) -> bool {
    for p in &step.predicates {
        /* The recogniser already confirmed the shape. */
        let ap = match match_attr_pred(p) {
            Some(ap) => ap,
            None => return false,
        };
        if !attr_pred_matches::<D>(doc, &ap, n, lax) {
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
/// On a match or none, the answer is the 0-or-1-node node-set.
#[allow(clippy::result_large_err)]
pub(crate) fn try_first_match<'e, 'd, D: Dom<'d>>(
    cx: &'e Context<'d, D>,
    names: &'e Names,
    doc: D,
    node: Option<D::Node>,
    ast: &Ast,
) -> Result<Option<Val>, Error> {
    let mut ev = Evaluation::new(cx, names, doc, None);
    let found = match first_match_walk::<D>(&mut ev, ast, node) {
        Ok(Some(found)) => found,
        Ok(None) => return Ok(None),
        Err(_) => return Err(ev.budget.take_error()),
    };
    let mut set = NodeSet::new();
    if let Some(n) = found {
        if set.push(D::token(n), &mut ev.budget).is_err() {
            return Err(ev.budget.take_error());
        }
    }
    Ok(Some(Val::NodeSet(set)))
}

fn first_match_walk<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    ast: &Ast,
    node: Option<D::Node>,
) -> EvalResult<Option<Option<D::Node>>> {
    let doc = ev.doc;
    let root = ast.root();
    let step = match first_recognise(root) {
        Some(s) => s,
        None => return Ok(None),
    };

    /* Compile the test as the step driver does, so the fast path stays
     * identical to the full evaluator down to the unknown-prefix error. The
     * test's axis is the recognised step's own (child:: in the `//X[@a]` form,
     * descendant:: otherwise); the walk below supplies the descendants. */
    let names: &'e Names = ev.names;
    let ct = CompiledTest::new(&step.test, step.axis, names, ev.cx.lax(), ev.budget.sink())?;

    let absolute = matches!(&root.kind, ExprKind::Path(p) if p.absolute);
    let start = if absolute {
        Some(doc.document_node())
    } else {
        node
    };
    let Some(start) = start else {
        return Ok(Some(None)); /* recognised; no context means no match */
    };

    let budget = &mut ev.budget;
    let flow = walk_descendants::<D, _, _>(doc, start, &mut |n| {
        if let Err(e) = budget.charge_op() {
            return ControlFlow::Break(Err(e));
        }
        if ct.matches(doc, n) && first_node_ok::<D>(doc, step, n, ct.lax()) {
            return ControlFlow::Break(Ok(n));
        }
        ControlFlow::Continue(())
    });
    match flow {
        ControlFlow::Continue(()) => Ok(Some(None)),
        ControlFlow::Break(found) => found.map(|n| Some(Some(n))),
    }
}

/* ---------- the expression evaluator ---------- */

fn eval_path<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    p: &Path,
    self_node: Option<D::Node>,
) -> EvalResult<Val<D::Node>> {
    let mut seed = NodeSet::new();
    let start = if p.absolute {
        Some(ev.doc.document_node())
    } else {
        self_node
    };
    if let Some(n) = start {
        seed.push(n, &mut ev.budget)?;
    }
    eval_steps::<D>(ev, &p.steps, &mut seed)
}

fn eval_filter<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    expr: &Expr,
    predicates: &[Expr],
    steps: &[Step],
    focus: &Focus<'d, D>,
) -> EvalResult<Val<D::Node>> {
    let mut primary = eval_node::<D>(ev, expr, focus)?;
    if !predicates.is_empty() {
        let Some(ns) = primary.as_nodeset_mut() else {
            return Err(err_setf!(
                ev.budget.sink(),
                XP_ERR_TYPE,
                "predicate applied to non-node-set"
            ));
        };
        apply_predicates::<D>(ev, predicates, ns)?;
    }
    if !steps.is_empty() {
        let Some(ns) = primary.as_nodeset_mut() else {
            return Err(err_setf!(
                ev.budget.sink(),
                XP_ERR_TYPE,
                "path applied to non-node-set"
            ));
        };
        let mut seed = core::mem::take(ns);
        return eval_steps::<D>(ev, steps, &mut seed);
    }
    Ok(primary)
}

/// A function call: the prefix resolved, the arguments evaluated, and the
/// call answered by the built-in library or, failing that, by the handler.
fn eval_fncall<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    prefix: Option<&[u8]>,
    name: &[u8],
    args: &[Expr],
    focus: &Focus<'d, D>,
) -> EvalResult<Val<D::Node>> {
    let names = ev.names;
    let ns_uri = match prefix {
        None => None,
        Some(prefix) => Some(names.resolve_prefix(prefix, ev.budget.sink())?),
    };

    /* The arguments are evaluated once and reused by either path. They are owned
     * here, so every way out - an argument failing part-way included - clears
     * them when `vals` drops. */
    let mut vals: Vec<Val<D::Node>> = Vec::new();
    if !args.is_empty() && vals.falloc_reserve_exact(args.len()).is_err() {
        return Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_OOM,
            "out of memory allocating function arguments"
        ));
    }
    for a in args {
        vals.push(eval_node::<D>(ev, a, focus)?);
    }

    if let Some(f) = funcs::lookup::<D>(ns_uri, name) {
        return f.call(ev, focus, &vals);
    }
    match ev.call_handler(focus, ns_uri, name, &vals)? {
        Some(v) => Ok(v),
        None => Err(err_setf!(
            ev.budget.sink(),
            XP_ERR_RUNTIME,
            "unknown function {}{}{}",
            Bytes(prefix.unwrap_or(&[])),
            if prefix.is_none() { "" } else { ":" },
            Bytes(name)
        )),
    }
}

impl<'e, 'd, D: Dom<'d>> Evaluation<'e, 'd, D> {
    /// Ask this evaluate's handler for a function with no built-in: `None`
    /// when there is no handler, or it has no such function.
    ///
    /// The handler works in tokens: the arguments are copied out as tokens, and
    /// its answer read back as this document's nodes - a resolver answers only
    /// nodes of this document (the bridge checks a handler's node before
    /// minting its token).
    fn call_handler(
        &mut self,
        focus: &Focus<'d, D>,
        ns_uri: Option<&[u8]>,
        local: &[u8],
        args: &[Val<D::Node>],
    ) -> EvalResult<Option<Val<D::Node>>> {
        let Some(handler) = self.handler else {
            return Ok(None);
        };
        let mut token_args: Vec<Val> = Vec::new();
        if token_args.falloc_reserve_exact(args.len()).is_err() {
            return Err(handler_oom(&mut self.budget));
        }
        for v in args {
            let Some(t) = val_copy_to_tokens::<D>(v) else {
                return Err(handler_oom(&mut self.budget));
            };
            token_args.push(t);
        }
        let call = ResolverCall {
            node: focus.node.map_or(Token::null(), D::token),
            pos: focus.pos,
            size: focus.size,
            ns_uri,
            local,
            args: &token_args,
        };
        match handler.resolve(&mut self.budget, &call)? {
            None => Ok(None),
            Some(v) => {
                let Some(mut v) = val_from_tokens::<D>(self.doc, v) else {
                    return Err(handler_oom(&mut self.budget));
                };
                if let Some(ns) = v.as_nodeset_mut() {
                    nodeset_unique_sorted::<D>(self, ns);
                }
                Ok(Some(v))
            }
        }
    }
}

#[cold]
fn handler_oom(budget: &mut Budget) -> Reported {
    err_setf!(
        budget.sink(),
        XP_ERR_OOM,
        "out of memory passing a function call to the handler"
    )
}

fn eval_binop<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    op: Op,
    lhs: &Expr,
    rhs: &Expr,
    focus: &Focus<'d, D>,
) -> EvalResult<Val<D::Node>> {
    let doc = ev.doc;

    /* and / or short-circuit. */
    if op == Op::Or || op == Op::And {
        let l = eval_node::<D>(ev, lhs, focus)?;
        let lb = val_to_boolean(&l);
        if (op == Op::Or && lb) || (op == Op::And && !lb) {
            return Ok(Val::boolean(lb));
        }
        let r = eval_node::<D>(ev, rhs, focus)?;
        return Ok(Val::boolean(val_to_boolean(&r)));
    }

    let l = eval_node::<D>(ev, lhs, focus)?;
    let r = eval_node::<D>(ev, rhs, focus)?;
    let (l, r): (&Val<D::Node>, &Val<D::Node>) = (&l, &r);

    match op {
        Op::Eq | Op::Ne => Ok(Val::boolean(compare_eq::<D>(ev, l, r, op)?)),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => Ok(Val::boolean(compare_rel::<D>(ev, l, r, op)?)),
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod => {
            let a = val_to_number_or_fail::<D>(doc, l, &mut ev.budget)?;
            let c = val_to_number_or_fail::<D>(doc, r, &mut ev.budget)?;
            Ok(Val::number(match op {
                Op::Add => a + c,
                Op::Sub => a - c,
                Op::Mul => a * c,
                Op::Div => a / c,
                _ => a % c,
            }))
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
fn eval_negate<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    x: &Expr,
    focus: &Focus<'d, D>,
) -> EvalResult<Val<D::Node>> {
    let doc = ev.doc;
    let v = eval_node::<D>(ev, x, focus)?;
    let d = val_to_number_or_fail::<D>(doc, &v, &mut ev.budget)?;
    Ok(Val::number(-d))
}

/// A string result copied from `bytes`.
fn string_value<N>(bytes: &[u8], err: ErrSink, what: &core::ffi::CStr) -> EvalResult<Val<N>> {
    Ok(Val::string(owned_copy(bytes, err, what)?))
}

/// The evaluator's only recursive function, and therefore the whole of "AST
/// recursion is bounded": one op and one recursion level are charged on entry
/// and the level is released at the single exit. Keeping it single-exit is what
/// makes that balance locally checkable.
fn eval_node<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    e: &Expr,
    focus: &Focus<'d, D>,
) -> EvalResult<Val<D::Node>> {
    ev.budget.charge_op()?;
    /* A refused entry is not counted, so returning here needs no release. */
    ev.budget.enter_recursion()?;
    let result = eval_node_inner::<D>(ev, e, focus);
    ev.budget.leave_recursion();
    result
}

fn eval_node_inner<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    e: &Expr,
    focus: &Focus<'d, D>,
) -> EvalResult<Val<D::Node>> {
    /* Hoisting: a context-independent subtree already computed in this evaluate
     * comes back as a clone, which keeps ownership clean - clearing either copy
     * is safe. */
    if let Some(slot) = e.memo {
        if let Some(v) = &ev.memo.0[slot as usize] {
            return val_clone(v, ev.budget.sink());
        }
    }

    let names = ev.names;
    let value = match &e.kind {
        ExprKind::LiteralStr(t) => {
            string_value(t, ev.budget.sink(), c"out of memory copying literal")
        }
        ExprKind::LiteralNum(d) => Ok(Val::number(*d)),
        ExprKind::VarRef { prefix, name } => match names.variable_text(prefix.as_deref(), name) {
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

/// Evaluate an AST over `doc` with `node` as the focus, on a fresh evaluation
/// that `handler` answers unknown functions for.
#[allow(clippy::result_large_err)]
pub(crate) fn eval_ast<'e, 'd, D: Dom<'d>>(
    cx: &'e Context<'d, D>,
    names: &'e Names,
    doc: D,
    node: Option<D::Node>,
    ast: &Ast,
    handler: Option<&'e dyn Resolver>,
) -> Result<Val, Error> {
    let mut ev = Evaluation::new(cx, names, doc, handler);
    let result = match Memo::new(ast.memo_slots(), ev.budget.sink()) {
        Ok(memo) => {
            ev.memo = memo;
            let focus = Focus {
                node,
                pos: 1,
                size: 1,
            };
            eval_node::<D>(&mut ev, ast.root(), &focus)
        }
        Err(e) => Err(e),
    };
    match result.map(val_to_tokens::<D>) {
        Ok(Some(v)) => Ok(v),
        Ok(None) => {
            let _ = err_setf!(
                ev.budget.sink(),
                XP_ERR_OOM,
                "out of memory returning the result"
            );
            Err(ev.budget.take_error())
        }
        Err(_) => Err(ev.budget.take_error()),
    }
}
