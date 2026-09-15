//! The passes run over a freshly parsed AST: the `//` peephole, then the
//! hoisting analysis that finds context-independent subtrees and gives the ones
//! worth remembering a memo slot.

use super::ast::{Ast, Axis, Expr, ExprKind, Step, TestKind};

/* ---------- the peephole: // fusion ---------- */

/// Collapse each pair of consecutive steps
///
/// ```text
/// (descendant-or-self, node(), no predicates)
/// (child,             X,       no predicates)
/// ```
///
/// into one `(descendant, X, no predicates)`.
///
/// Safe per §2.5 only when the child step has no predicates: otherwise `//X[1]`
/// would change meaning, from "the first X of each parent" to "the first X in
/// document order". The synthesised `//` step never has predicates by
/// construction, so only the child step's list has to be checked.
fn fuse_descendant_or_self(steps: &mut Vec<Step>) {
    let mut i = 0;
    while i + 1 < steps.len() {
        let (s, next) = (&steps[i], &steps[i + 1]);
        let fusable = s.axis == Axis::DescendantOrSelf
            && s.test.kind == TestKind::Node
            && s.test.prefix.is_none()
            && s.predicates.is_empty()
            && next.axis == Axis::Child
            && next.predicates.is_empty();
        if fusable {
            /* Drop the descendant-or-self step and promote the child step. */
            steps.remove(i);
            steps[i].axis = Axis::Descendant;
        }
        i += 1;
    }
}

fn peephole_steps(steps: &mut Vec<Step>) {
    fuse_descendant_or_self(steps);
    for s in steps {
        for p in &mut s.predicates {
            apply_peephole(p);
        }
    }
}

pub fn apply_peephole(e: &mut Expr) {
    match &mut e.kind {
        ExprKind::FnCall { args, .. } => {
            for a in args {
                apply_peephole(a);
            }
        }
        ExprKind::Negate(x) => apply_peephole(x),
        ExprKind::BinOp { lhs, rhs, .. } => {
            apply_peephole(lhs);
            apply_peephole(rhs);
        }
        ExprKind::Path(p) => peephole_steps(&mut p.steps),
        ExprKind::Filter {
            expr,
            predicates,
            steps,
        } => {
            apply_peephole(expr);
            for p in predicates {
                apply_peephole(p);
            }
            peephole_steps(steps);
        }
        ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_) | ExprKind::VarRef { .. } => {}
    }
}

/* ---------- hoisting ---------- */

/// The pure XPath 1.0 built-ins safe to hoist when all their arguments are
/// context-independent. Listed explicitly to keep the set conservative:
/// anything that reads the context node (last, position, the zero-argument
/// string / normalize-space / local-name, lang) or that may depend on dynamic
/// state (id, handler-routed calls) is deliberately absent.
fn is_pure_builtin(name: &[u8], nargs: usize) -> bool {
    if nargs == 0 {
        /* These read no input at all. */
        return name == b"true" || name == b"false";
    }
    matches!(
        name,
        b"count"
            | b"string-length"
            | b"number"
            | b"boolean"
            | b"not"
            | b"floor"
            | b"ceiling"
            | b"round"
            | b"sum"
            | b"concat"
            | b"starts-with"
            | b"contains"
            | b"substring-before"
            | b"substring-after"
            | b"substring"
            | b"translate"
    )
}

/// Record on every subtree whether it evaluates to the same value wherever it
/// appears in one evaluate, and return the answer for `e`.
///
/// Bottom-up in one walk, so each node is visited once however deep the tree.
fn mark_context_independent(e: &mut Expr) -> bool {
    let ci = match &mut e.kind {
        ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_) => true,
        /* Conservative: a variable is not hoisted even though §1 fixes it per
         * evaluation. */
        ExprKind::VarRef { .. } => false,
        ExprKind::FnCall { prefix, name, args } => {
            let mut all = true;
            for a in args.iter_mut() {
                all &= mark_context_independent(a);
            }
            /* A prefix means handler-routed or a namespaced builtin, neither
             * of which is hoistable. */
            prefix.is_none() && is_pure_builtin(name, args.len()) && all
        }
        ExprKind::Negate(x) => mark_context_independent(x),
        ExprKind::BinOp { lhs, rhs, .. } => {
            let l = mark_context_independent(lhs);
            let r = mark_context_independent(rhs);
            l && r
        }
        /* An absolute path is context-independent: its seed is the document root
         * whatever the outer context. A relative one uses the outer context node
         * and is not hoistable. Predicates inside a path are evaluated against the
         * path's own context, so their position() and last() do not leak -
         * recurse so pure sub-expressions still get marked. */
        ExprKind::Path(p) => {
            mark_step_predicates(&mut p.steps);
            p.absolute
        }
        /* Conservative: filter expressions are not hoisted. */
        ExprKind::Filter {
            expr,
            predicates,
            steps,
        } => {
            mark_context_independent(expr);
            for p in predicates {
                mark_context_independent(p);
            }
            mark_step_predicates(steps);
            false
        }
    };
    e.context_independent = ci;
    ci
}

fn mark_step_predicates(steps: &mut [Step]) {
    for s in steps {
        for p in &mut s.predicates {
            mark_context_independent(p);
        }
    }
}

/// Give a memo slot to every subtree for which remembering the value can save
/// work, and return how many were given. `mark_context_independent` must have
/// run over `root`.
///
/// That is a context-independent subtree that
///
/// - sits inside a predicate, the only place an expression is evaluated more
///   than once in one evaluate (once per candidate node) - outside one, the
///   remembered value would never be read;
/// - is not a literal, which costs no more to evaluate than to copy back; and
/// - is the largest such subtree: under a remembered parent, a child is never
///   evaluated a second time.
///
/// None of this changes an answer or the budget charged: `eval_node` charges
/// its op before consulting the table, and a subtree that is left out is one
/// whose remembered value would not have been read.
fn assign_memo_slots(root: &mut Expr) -> u32 {
    let mut next = 0u32;
    assign(root, false, true, &mut next);
    next
}

fn assign(e: &mut Expr, in_predicate: bool, parent_ci: bool, next: &mut u32) {
    let ci = e.context_independent;
    let literal = matches!(e.kind, ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_));
    if in_predicate && ci && !parent_ci && !literal {
        if let Some(following) = next.checked_add(1) {
            e.memo = Some(*next);
            *next = following;
        }
    }
    match &mut e.kind {
        ExprKind::FnCall { args, .. } => {
            for a in args {
                assign(a, in_predicate, ci, next);
            }
        }
        ExprKind::Negate(x) => assign(x, in_predicate, ci, next),
        ExprKind::BinOp { lhs, rhs, .. } => {
            assign(lhs, in_predicate, ci, next);
            assign(rhs, in_predicate, ci, next);
        }
        ExprKind::Path(p) => assign_step_predicates(&mut p.steps, next),
        ExprKind::Filter {
            expr,
            predicates,
            steps,
        } => {
            assign(expr, in_predicate, false, next);
            for p in predicates {
                assign(p, true, false, next);
            }
            assign_step_predicates(steps, next);
        }
        ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_) | ExprKind::VarRef { .. } => {}
    }
}

/// A predicate is evaluated once per candidate node, and its parent is a step,
/// not an expression - so its root is always the largest subtree there.
fn assign_step_predicates(steps: &mut [Step], next: &mut u32) {
    for s in steps {
        for p in &mut s.predicates {
            assign(p, true, false, next);
        }
    }
}

/// Run the passes over a parsed root and wrap it.
pub fn finish(mut root: Expr) -> Ast {
    /* Peephole first, so the hoisting pass sees the rewritten step structure. */
    apply_peephole(&mut root);
    mark_context_independent(&mut root);
    let slots = assign_memo_slots(&mut root);
    Ast::with_memo_slots(root, slots)
}
