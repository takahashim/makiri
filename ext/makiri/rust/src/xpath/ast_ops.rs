//! Building, destroying and rewriting the C AST (mkr_xpath_shared.c's half of
//! it): the one node factory, the destructors, the hoisting pass that marks
//! context-independent subtrees, and the `//` peephole.
//!
//! Separate from `ast.rs` because these export C symbols that
//! mkr_xpath_shared.c still defines unless this feature replaces it, while the
//! views next door export nothing and are always available.

/* Each takes AST pointers its caller already holds; the contract is the one at
 * the declaration in mkr_xpath_internal.h. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::ast_view::{path_steps, step_preds};
use crate::err_setf;
use core::ffi::c_void;
use core::ptr;

/// The one AST factory: charges the node budget, then hands back a zeroed node
/// with its kind set. The XPath parser and the CSS lowering both go through it,
/// which is what keeps `mkr_node_free` able to take apart whatever either built.
pub unsafe fn mkr_node_alloc(limits: *mut Limits, err: *mut Error, kind: u32) -> *mut Node {
    if mkr_limit_ast_node(limits, err) != 0 {
        return ptr::null_mut();
    }
    let n = mkr_callocarray(1, core::mem::size_of::<Node>()) as *mut Node;
    if n.is_null() {
        err_setf!(err, XP_ERR_OOM, "out of memory allocating AST node");
        return ptr::null_mut();
    }
    (*n).kind = kind;
    n
}

pub unsafe fn mkr_step_clear(s: *mut Step) {
    if s.is_null() {
        return;
    }
    mkr_owned_text_clear(&raw mut (*s).test.prefix);
    mkr_owned_text_clear(&raw mut (*s).test.local);
    mkr_owned_text_clear(&raw mut (*s).test.pi_target);
    for &p in step_preds(s) {
        mkr_node_free(p);
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
        mkr_mark_context_independent(p);
    }
}

unsafe fn text_bytes<'a>(t: OwnedText) -> &'a [u8] {
    if t.ptr.is_null() || t.len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(t.ptr as *const u8, t.len)
    }
}

unsafe fn is_ci(n: *const Node) -> bool {
    !n.is_null() && (*n).is_context_independent != 0
}

pub unsafe fn mkr_mark_context_independent(n: *mut Node) {
    if n.is_null() {
        return;
    }
    let ci = match (*n).kind {
        NK_LITERAL_STR | NK_LITERAL_NUM => true,
        /* Conservative: a variable is not hoisted even though §1 fixes it per
         * evaluation. */
        NK_VARREF => false,
        NK_FNCALL => {
            let call = &raw const (*n).u.fncall;
            let args = if (*call).nargs == 0 {
                &[][..]
            } else {
                core::slice::from_raw_parts((*call).args, (*call).nargs)
            };
            /* Recurse first, so subtrees get their own marks even when this call
             * is not itself hoistable. */
            for &a in args {
                mkr_mark_context_independent(a);
            }
            /* A prefix means handler-routed or a namespaced builtin, neither
             * of which is hoistable. */
            (*call).prefix.ptr.is_null()
                && is_pure_builtin(text_bytes((*call).name), args.len())
                && args.iter().all(|&a| is_ci(a))
        }
        NK_UNARY => {
            mkr_mark_context_independent((*n).u.unary.expr);
            is_ci((*n).u.unary.expr)
        }
        NK_BINOP => {
            mkr_mark_context_independent((*n).u.binop.lhs);
            mkr_mark_context_independent((*n).u.binop.rhs);
            is_ci((*n).u.binop.lhs) && is_ci((*n).u.binop.rhs)
        }
        NK_PATH => {
            /* An absolute path is context-independent: its seed is the document
             * root whatever the outer context. A relative one uses the outer
             * context node and is not hoistable. Predicates inside a path are
             * evaluated against the path's own context, so their position() and
             * last() do not leak - recurse so pure sub-expressions still get
             * marked. */
            for s in path_steps((*n).u.path.steps, (*n).u.path.nsteps) {
                mark_step_predicates(s);
            }
            (*n).u.path.absolute != 0
        }
        NK_FILTER => {
            /* Conservative: filter expressions are not hoisted. */
            let f = &raw const (*n).u.filter;
            mkr_mark_context_independent((*f).expr);
            if (*f).npreds > 0 {
                for &p in core::slice::from_raw_parts((*f).preds, (*f).npreds) {
                    mkr_mark_context_independent(p);
                }
            }
            for s in path_steps((*f).path_steps, (*f).npath) {
                mark_step_predicates(s);
            }
            false
        }
        _ => false,
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
            && all[r].axis == AXIS_DESCENDANT_OR_SELF
            && all[r].test.kind == NT_NODE
            && all[r].test.prefix.ptr.is_null()
            && all[r].npredicates == 0
            && all[r + 1].axis == AXIS_CHILD
            && all[r + 1].npredicates == 0;
        if fusable {
            /* Drop the descendant-or-self step and promote the child step. */
            mkr_step_clear(&mut all[r]);
            all[w] = all[r + 1];
            ptr::write_bytes(&mut all[r + 1], 0, 1);
            all[w].axis = AXIS_DESCENDANT;
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
        mkr_apply_peephole(p);
    }
}

pub unsafe fn mkr_apply_peephole(n: *mut Node) {
    if n.is_null() {
        return;
    }
    match (*n).kind {
        NK_FNCALL => {
            let call = &raw const (*n).u.fncall;
            if (*call).nargs > 0 {
                for &a in core::slice::from_raw_parts((*call).args, (*call).nargs) {
                    mkr_apply_peephole(a);
                }
            }
        }
        NK_UNARY => mkr_apply_peephole((*n).u.unary.expr),
        NK_BINOP => {
            mkr_apply_peephole((*n).u.binop.lhs);
            mkr_apply_peephole((*n).u.binop.rhs);
        }
        NK_PATH => {
            let p = &raw mut (*n).u.path;
            fuse_descendant_or_self((*p).steps, &raw mut (*p).nsteps);
            for s in path_steps((*p).steps, (*p).nsteps) {
                peephole_step_predicates(s);
            }
        }
        NK_FILTER => {
            let f = &raw mut (*n).u.filter;
            mkr_apply_peephole((*f).expr);
            if (*f).npreds > 0 {
                for &p in core::slice::from_raw_parts((*f).preds, (*f).npreds) {
                    mkr_apply_peephole(p);
                }
            }
            fuse_descendant_or_self((*f).path_steps, &raw mut (*f).npath);
            for s in path_steps((*f).path_steps, (*f).npath) {
                peephole_step_predicates(s);
            }
        }
        _ => {}
    }
}

/* ---------- memos and destruction ---------- */

unsafe fn clear_memos_step(s: *const Step) {
    for &p in step_preds(s) {
        mkr_node_clear_memos(p);
    }
}

pub unsafe fn mkr_node_clear_memos(n: *mut Node) {
    if n.is_null() {
        return;
    }
    if (*n).memoized != 0 {
        mkr_val_clear(&raw mut (*n).memo_value);
        (*n).memoized = 0;
    }
    match (*n).kind {
        NK_FNCALL => {
            let call = &raw const (*n).u.fncall;
            if (*call).nargs > 0 {
                for &a in core::slice::from_raw_parts((*call).args, (*call).nargs) {
                    mkr_node_clear_memos(a);
                }
            }
        }
        NK_UNARY => mkr_node_clear_memos((*n).u.unary.expr),
        NK_BINOP => {
            mkr_node_clear_memos((*n).u.binop.lhs);
            mkr_node_clear_memos((*n).u.binop.rhs);
        }
        NK_PATH => {
            for s in path_steps((*n).u.path.steps, (*n).u.path.nsteps) {
                clear_memos_step(s);
            }
        }
        NK_FILTER => {
            let f = &raw const (*n).u.filter;
            mkr_node_clear_memos((*f).expr);
            if (*f).npreds > 0 {
                for &p in core::slice::from_raw_parts((*f).preds, (*f).npreds) {
                    mkr_node_clear_memos(p);
                }
            }
            for s in path_steps((*f).path_steps, (*f).npath) {
                clear_memos_step(s);
            }
        }
        _ => {}
    }
}

pub unsafe fn mkr_node_free(n: *mut Node) {
    if n.is_null() {
        return;
    }
    /* Free any memoized value first; the clear is idempotent. */
    if (*n).memoized != 0 {
        mkr_val_clear(&raw mut (*n).memo_value);
        (*n).memoized = 0;
    }
    match (*n).kind {
        NK_LITERAL_STR => mkr_owned_text_clear(&raw mut (*n).u.literal),
        NK_LITERAL_NUM => {}
        NK_VARREF => {
            mkr_owned_text_clear(&raw mut (*n).u.varref.prefix);
            mkr_owned_text_clear(&raw mut (*n).u.varref.name);
        }
        NK_FNCALL => {
            let call = &raw mut (*n).u.fncall;
            mkr_owned_text_clear(&raw mut (*call).prefix);
            mkr_owned_text_clear(&raw mut (*call).name);
            if (*call).nargs > 0 {
                for &a in core::slice::from_raw_parts((*call).args, (*call).nargs) {
                    mkr_node_free(a);
                }
            }
            if !(*call).args.is_null() {
                free_c((*call).args as *mut c_void);
            }
        }
        NK_UNARY => mkr_node_free((*n).u.unary.expr),
        NK_BINOP => {
            mkr_node_free((*n).u.binop.lhs);
            mkr_node_free((*n).u.binop.rhs);
        }
        NK_PATH => {
            let p = &raw mut (*n).u.path;
            for s in steps_mut((*p).steps, (*p).nsteps) {
                mkr_step_clear(s);
            }
            if !(*p).steps.is_null() {
                free_c((*p).steps as *mut c_void);
            }
        }
        NK_FILTER => {
            let f = &raw mut (*n).u.filter;
            mkr_node_free((*f).expr);
            if (*f).npreds > 0 {
                for &p in core::slice::from_raw_parts((*f).preds, (*f).npreds) {
                    mkr_node_free(p);
                }
            }
            if !(*f).preds.is_null() {
                free_c((*f).preds as *mut c_void);
            }
            for s in steps_mut((*f).path_steps, (*f).npath) {
                mkr_step_clear(s);
            }
            if !(*f).path_steps.is_null() {
                free_c((*f).path_steps as *mut c_void);
            }
        }
        _ => {}
    }
    free_c(n as *mut c_void);
}

extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
