//! Building, destroying and rewriting the AST: the one node factory, the
//! destructors, the hoisting pass that marks context-independent subtrees, and
//! the `//` peephole.
//!
//! Separate from `ast_view.rs`, whose views only borrow: everything here
//! allocates, frees or rewrites.

/* Each takes AST pointers its caller already holds. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::ast_view::{path_steps, step_preds};
use super::own::Ast;
use crate::err_setf;
use crate::falloc::raw::callocarray;
use core::ffi::c_void;
use core::ptr;
use core::ptr::NonNull;

/// The one AST factory: charges the node budget, then hands back a zeroed node
/// with its kind set. The XPath parser and the CSS lowering both go through it,
/// which is what keeps `node_free` able to take apart whatever either built.
///
/// # Safety
/// `budget` must be live.
pub(crate) unsafe fn node_alloc(budget: *mut Budget, kind: NodeKind) -> Result<Ast, Reported> {
    let err = budget_sink(budget);
    limit_ast_node(budget)?;
    let Some(n) = NonNull::new(callocarray(1, core::mem::size_of::<Node>()) as *mut Node) else {
        return Err(err_setf!(
            err,
            XP_ERR_OOM,
            "out of memory allocating AST node"
        ));
    };
    (*n.as_ptr()).kind = kind;
    // SAFETY: a fresh zeroed node, owned by nothing else.
    Ok(Ast::from_non_null(n))
}

pub unsafe fn step_clear(s: *mut Step) {
    if s.is_null() {
        return;
    }
    (*s).test.prefix.clear();
    (*s).test.local.clear();
    (*s).test.pi_target.clear();
    for &p in step_preds(s) {
        node_free(p);
    }
    if !(*s).predicates.is_null() {
        free_c((*s).predicates as *mut c_void);
    }
    ptr::write_bytes(s, 0, 1);
}

/// Every step of a path, as a mutable slice. Separate from `path_steps` because
/// the peephole rewrites in place.
unsafe fn steps_mut<'a>(steps: *mut Step, n: usize) -> &'a mut [Step] {
    if n == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut(steps, n)
    }
}

/// A node-pointer array as a slice; empty when there are none.
unsafe fn node_list<'a>(p: *mut *mut Node, n: usize) -> &'a [*mut Node] {
    if n == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p, n)
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

unsafe fn mark_step_predicates(s: *const Step) {
    for &p in step_preds(s) {
        mark_context_independent(p);
    }
}

unsafe fn text_bytes<'a>(t: TextSlot) -> &'a [u8] {
    t.as_bytes()
}

unsafe fn is_ci(n: *const Node) -> bool {
    !n.is_null() && (*n).is_context_independent != 0
}

pub unsafe fn mark_context_independent(n: *mut Node) {
    if n.is_null() {
        return;
    }
    let ci = match Node::view(n) {
        NodeRef::LiteralStr(_) | NodeRef::LiteralNum(_) => true,
        /* Conservative: a variable is not hoisted even though §1 fixes it per
         * evaluation. */
        NodeRef::VarRef(_) => false,
        NodeRef::FnCall(call) => {
            let args = node_list(call.args, call.nargs);
            /* Recurse first, so subtrees get their own marks even when this call
             * is not itself hoistable. */
            for &a in args {
                mark_context_independent(a);
            }
            /* A prefix means handler-routed or a namespaced builtin, neither
             * of which is hoistable. */
            call.prefix.is_absent()
                && is_pure_builtin(text_bytes(call.name), args.len())
                && args.iter().all(|&a| is_ci(a))
        }
        NodeRef::Unary(u) => {
            mark_context_independent(u.expr);
            is_ci(u.expr)
        }
        NodeRef::BinOp(b) => {
            mark_context_independent(b.lhs);
            mark_context_independent(b.rhs);
            is_ci(b.lhs) && is_ci(b.rhs)
        }
        NodeRef::Path(p) => {
            /* An absolute path is context-independent: its seed is the document
             * root whatever the outer context. A relative one uses the outer
             * context node and is not hoistable. Predicates inside a path are
             * evaluated against the path's own context, so their position() and
             * last() do not leak - recurse so pure sub-expressions still get
             * marked. */
            for s in path_steps(p.steps, p.nsteps) {
                mark_step_predicates(s);
            }
            p.absolute != 0
        }
        NodeRef::Filter(f) => {
            /* Conservative: filter expressions are not hoisted. */
            mark_context_independent(f.expr);
            for &p in node_list(f.preds, f.npreds) {
                mark_context_independent(p);
            }
            for s in path_steps(f.path_steps, f.npath) {
                mark_step_predicates(s);
            }
            false
        }
    };
    (*n).is_context_independent = u8::from(ci);
}

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
unsafe fn fuse_descendant_or_self(steps: *mut Step, nsteps: *mut usize) {
    if steps.is_null() || *nsteps < 2 {
        return;
    }
    let n = *nsteps;
    let all = steps_mut(steps, n);
    let (mut w, mut r) = (0usize, 0usize);
    while r < n {
        let fusable = r + 1 < n
            && all[r].axis == Axis::DescendantOrSelf
            && all[r].test.kind == TestKind::Node
            && all[r].test.prefix.is_absent()
            && all[r].npredicates == 0
            && all[r + 1].axis == Axis::Child
            && all[r + 1].npredicates == 0;
        if fusable {
            /* Drop the descendant-or-self step and promote the child step. */
            step_clear(&mut all[r]);
            all[w] = all[r + 1];
            ptr::write_bytes(&mut all[r + 1], 0, 1);
            all[w].axis = Axis::Descendant;
            w += 1;
            r += 2;
        } else {
            if w != r {
                all[w] = all[r];
                ptr::write_bytes(&mut all[r], 0, 1);
            }
            w += 1;
            r += 1;
        }
    }
    *nsteps = w;
}

unsafe fn peephole_step_predicates(s: *const Step) {
    for &p in step_preds(s) {
        apply_peephole(p);
    }
}

pub unsafe fn apply_peephole(n: *mut Node) {
    if n.is_null() {
        return;
    }
    match Node::view_mut(n) {
        NodeMut::FnCall(call) => {
            for &a in node_list(call.args, call.nargs) {
                apply_peephole(a);
            }
        }
        NodeMut::Unary(u) => apply_peephole(u.expr),
        NodeMut::BinOp(b) => {
            apply_peephole(b.lhs);
            apply_peephole(b.rhs);
        }
        NodeMut::Path(p) => {
            fuse_descendant_or_self(p.steps, &mut p.nsteps);
            for s in path_steps(p.steps, p.nsteps) {
                peephole_step_predicates(s);
            }
        }
        NodeMut::Filter(f) => {
            apply_peephole(f.expr);
            for &p in node_list(f.preds, f.npreds) {
                apply_peephole(p);
            }
            fuse_descendant_or_self(f.path_steps, &mut f.npath);
            for s in path_steps(f.path_steps, f.npath) {
                peephole_step_predicates(s);
            }
        }
        _ => {}
    }
}

/* ---------- memos and destruction ---------- */

unsafe fn clear_memos_step(s: *const Step) {
    for &p in step_preds(s) {
        node_clear_memos(p);
    }
}

pub unsafe fn node_clear_memos(n: *mut Node) {
    if n.is_null() {
        return;
    }
    if (*n).memoized != 0 {
        val_clear(&raw mut (*n).memo_value);
        (*n).memoized = 0;
    }
    match Node::view(n) {
        NodeRef::FnCall(call) => {
            for &a in node_list(call.args, call.nargs) {
                node_clear_memos(a);
            }
        }
        NodeRef::Unary(u) => node_clear_memos(u.expr),
        NodeRef::BinOp(b) => {
            node_clear_memos(b.lhs);
            node_clear_memos(b.rhs);
        }
        NodeRef::Path(p) => {
            for s in path_steps(p.steps, p.nsteps) {
                clear_memos_step(s);
            }
        }
        NodeRef::Filter(f) => {
            node_clear_memos(f.expr);
            for &p in node_list(f.preds, f.npreds) {
                node_clear_memos(p);
            }
            for s in path_steps(f.path_steps, f.npath) {
                clear_memos_step(s);
            }
        }
        _ => {}
    }
}

pub unsafe fn node_free(n: *mut Node) {
    if n.is_null() {
        return;
    }
    /* Free any memoized value first; the clear is idempotent. */
    if (*n).memoized != 0 {
        val_clear(&raw mut (*n).memo_value);
        (*n).memoized = 0;
    }
    match Node::view_mut(n) {
        NodeMut::LiteralStr(t) => t.clear(),
        NodeMut::LiteralNum(_) => {}
        NodeMut::VarRef(v) => {
            v.prefix.clear();
            v.name.clear();
        }
        NodeMut::FnCall(call) => {
            call.prefix.clear();
            call.name.clear();
            for &a in node_list(call.args, call.nargs) {
                node_free(a);
            }
            if !call.args.is_null() {
                free_c(call.args as *mut c_void);
            }
        }
        NodeMut::Unary(u) => node_free(u.expr),
        NodeMut::BinOp(b) => {
            node_free(b.lhs);
            node_free(b.rhs);
        }
        NodeMut::Path(p) => {
            for s in steps_mut(p.steps, p.nsteps) {
                step_clear(s);
            }
            if !p.steps.is_null() {
                free_c(p.steps as *mut c_void);
            }
        }
        NodeMut::Filter(f) => {
            node_free(f.expr);
            for &p in node_list(f.preds, f.npreds) {
                node_free(p);
            }
            if !f.preds.is_null() {
                free_c(f.preds as *mut c_void);
            }
            for s in steps_mut(f.path_steps, f.npath) {
                step_clear(s);
            }
            if !f.path_steps.is_null() {
                free_c(f.path_steps as *mut c_void);
            }
        }
    }
    free_c(n as *mut c_void);
}

extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
