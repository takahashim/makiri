//! The pass run over a freshly parsed AST: the `//` peephole and the hoisting
//! analysis that finds context-independent subtrees and gives the ones worth
//! remembering a memo slot.

#![forbid(unsafe_code)]

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

/// Run the passes over a parsed root and wrap it.
///
/// One walk does all of it, each node visited once however deep the tree: the
/// `//` fusion, whether each subtree is context-independent, and the memo
/// slots.
///
/// A slot goes to a context-independent subtree that
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
pub fn finish(mut root: Expr) -> Ast {
    let mut slots = 0u32;
    prepare(&mut root, false, &mut slots);
    Ast::with_memo_slots(root, slots)
}

/// Prepare `e`'s subtree, record whether `e` is context-independent and return
/// it. The caller decides `e`'s own slot, since that depends on the parent.
fn prepare(e: &mut Expr, in_predicate: bool, slots: &mut u32) -> bool {
    let ci = match &mut e.kind {
        ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_) => true,
        /* Conservative: a variable is not hoisted even though §1 fixes it per
         * evaluation. */
        ExprKind::VarRef { .. } => false,
        ExprKind::FnCall { prefix, name, args } => {
            let mut all = true;
            for a in args.iter_mut() {
                all &= prepare(a, in_predicate, slots);
            }
            /* A prefix means handler-routed or a namespaced builtin, neither
             * of which is hoistable. */
            let ci = prefix.is_none() && is_pure_builtin(name, args.len()) && all;
            if !ci && in_predicate {
                for a in args.iter_mut() {
                    give_slot(a, slots);
                }
            }
            ci
        }
        /* A negation is context-independent exactly when its operand is, so the
         * operand is never the largest such subtree. */
        ExprKind::Negate(x) => prepare(x, in_predicate, slots),
        ExprKind::BinOp { lhs, rhs, .. } => {
            let l = prepare(lhs, in_predicate, slots);
            let r = prepare(rhs, in_predicate, slots);
            let ci = l && r;
            if !ci && in_predicate {
                give_slot(lhs, slots);
                give_slot(rhs, slots);
            }
            ci
        }
        /* An absolute path is context-independent: its seed is the document root
         * whatever the outer context. A relative one uses the outer context node
         * and is not hoistable. Predicates inside a path are evaluated against the
         * path's own context, so their position() and last() do not leak. */
        ExprKind::Path(p) => {
            prepare_steps(&mut p.steps, slots);
            p.absolute
        }
        /* Conservative: filter expressions are not hoisted. */
        ExprKind::Filter {
            expr,
            predicates,
            steps,
        } => {
            prepare(expr, in_predicate, slots);
            if in_predicate {
                give_slot(expr, slots);
            }
            for p in predicates.iter_mut() {
                prepare(p, true, slots);
                give_slot(p, slots);
            }
            prepare_steps(steps, slots);
            false
        }
    };
    e.context_independent = ci;
    ci
}

/// Fuse a path's steps, then prepare their predicates. A predicate is evaluated
/// once per candidate node and its parent is a step, not an expression, so its
/// root is always the largest subtree there.
fn prepare_steps(steps: &mut Vec<Step>, slots: &mut u32) {
    fuse_descendant_or_self(steps);
    for s in steps.iter_mut() {
        for p in s.predicates.iter_mut() {
            prepare(p, true, slots);
            give_slot(p, slots);
        }
    }
}

/// A slot for an already-prepared subtree that is context-independent and not
/// a literal. Past `u32::MAX` slots the subtree is simply not remembered.
fn give_slot(e: &mut Expr, slots: &mut u32) {
    let literal = matches!(e.kind, ExprKind::LiteralStr(_) | ExprKind::LiteralNum(_));
    if !e.context_independent || literal {
        return;
    }
    if let Some(following) = slots.checked_add(1) {
        e.memo = Some(*slots);
        *slots = following;
    }
}
