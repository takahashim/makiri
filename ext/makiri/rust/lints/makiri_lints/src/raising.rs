//! Which of Ruby's C functions can raise, decided in one table, and the two
//! rules that follow from it.
//!
//! A raise is a `longjmp` over every Rust frame up to the nearest
//! `rb_protect`, undefined behaviour across a `Drop`, so every C call that
//! can raise runs inside the closure handed to `nupur::raw`'s `call`. The
//! table here is of the functions that CANNOT raise, each with its reason;
//! every other function of rb-sys is taken to raise - so a function nobody
//! has classified is refused until someone does, where a list of the ones
//! that raise let a missing entry (`rb_undef_alloc_func`, `rb_obj_freeze`)
//! through unseen.
//!
//! - `raising_call_outside_protect`: a function not in the table, called
//!   outside `call`'s closure. Exempt is `raw::protect`, the protection
//!   itself, `$!` and the calls that raise on purpose. A function that
//!   raises only when handed what a caller rules out (`rb_obj_is_kind_of`
//!   with a non-class) stays out of the table: its call site is an `allow`
//!   that says why.
//! - `unraising_call_in_raw`: what `nupur::raw` holds, read from the code. A
//!   call of Ruby's C in `raw` outside `call` (a read that raises nothing
//!   belongs with the values, `rb::value`), and a `call` whose closure calls
//!   nothing that can raise (a protection that protects nothing): each is
//!   the table and the code disagreeing about a function.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::Res;
use rustc_hir::{Expr, ExprKind, HirId, Node};
use rustc_lint::LateContext;
use std::ops::ControlFlow;

use crate::boundaries::{callee, is_extension, module_of, path_of};
use crate::{RAISING_CALL_OUTSIDE_PROTECT, UNRAISING_CALL_IN_RAW};

/// Ruby's C functions that raise nothing, whatever a caller of the layer
/// hands them.
const NON_RAISING: &[&str] = &[
    /* Reads of the thread and its current frame. */
    "rb_errinfo",
    "rb_block_given_p",
    "rb_keyword_given_p",
    "ruby_native_thread_p",
    "ruby_stack_check",
    /* The `2` variant leaves a pending interrupt pending, where the plain
     * one raises it as it returns. */
    "rb_thread_call_without_gvl2",
    /* The GC: marking, freeing, and a counter (which may start a
     * collection; a collection raises nothing). */
    "rb_gc_mark",
    "ruby_xfree",
    "rb_gc_adjust_memory_usage",
    /* A check: false for a value of any other type. */
    "rb_typeddata_is_kind_of",
    /* Reads of an object: an ivar (nil when unset), an Array element (nil
     * past the end), a Bignum's size. */
    "rb_attr_get",
    "rb_ary_entry",
    "rb_absint_size",
    /* An object's class field, written: hiding and showing. */
    "rb_obj_hide",
    "rb_obj_reveal",
    /* Encodings: the builtin three and their indices, and lookups by index
     * (NULL or -1 for none). */
    "rb_utf8_encoding",
    "rb_usascii_encoding",
    "rb_ascii8bit_encoding",
    "rb_utf8_encindex",
    "rb_usascii_encindex",
    "rb_ascii8bit_encindex",
    "rb_enc_get",
    "rb_enc_get_index",
    "rb_enc_from_index",
];

/// Where the protection, `$!` and the raises on purpose live.
const PROTECT: &str = "makiri::nupur::raw::protect";

/// The modules held to `unraising_call_in_raw`: `raw` itself, and the
/// self-test's stand-in.
const RAW: &[&str] = &["makiri::nupur::raw", "makiri::nupur::lint_probe::raw"];

/// The protected call, and the self-test's stand-in.
const CALL: &[&str] = &[
    "makiri::nupur::raw::protect::call",
    "makiri::nupur::lint_probe::raw::call",
];

/// A function of rb-sys, by name, when `path` names one.
fn rb_sys_function(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("rb_sys::")?;
    if rest.starts_with("stable_api") {
        /* The stable API's inline reads, which raise nothing. */
        return Some("");
    }
    rest.rsplit("::").next()
}

fn may_raise(name: &str) -> bool {
    !name.is_empty() && !NON_RAISING.contains(&name)
}

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    check_protected_closure(cx, expr);
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let path = path_of(cx, did);
    let Some(name) = rb_sys_function(&path) else {
        return;
    };
    let module = module_of(cx, expr.hir_id);
    let in_protect = module == PROTECT || module.starts_with(&format!("{PROTECT}::"));
    let inside = inside_call(cx, expr.hir_id);
    if may_raise(name) && !inside && !in_protect {
        span_lint_and_help(
            cx,
            RAISING_CALL_OUTSIDE_PROTECT,
            expr.span.source_callsite(),
            format!("`{name}` can raise, and is called outside `raw::call`"),
            None,
            "a raise is a longjmp over every Rust frame up to the nearest rb_protect; make it \
             a nupur::raw wrapper that runs it inside call(|| ...) - or, for a function that \
             cannot raise, add it to makiri_lints' NON_RAISING with the reason",
        );
    }
    if RAW.contains(&module.as_str()) && !inside {
        let what = if name.is_empty() {
            "the stable API"
        } else {
            name
        };
        span_lint_and_help(
            cx,
            UNRAISING_CALL_IN_RAW,
            expr.span.source_callsite(),
            format!("`{what}` is called in `raw` outside `call`"),
            None,
            "`raw` holds the C calls made inside call(|| ...); a read that raises nothing \
             belongs with the values (rb::value), and the protection itself in raw::protect",
        );
    }
}

/// A `call` whose closure calls nothing of Ruby's C that can raise.
fn check_protected_closure<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let ExprKind::Call(f, [arg]) = expr.kind else {
        return;
    };
    if !is_call(cx, f) {
        return;
    }
    let ExprKind::Closure(closure) = arg.kind else {
        return;
    };
    let body = cx.tcx.hir_body(closure.body);
    let raises = clippy_utils::visitors::for_each_expr(cx.tcx, body.value, |e| {
        match callee(cx, e).map(|did| path_of(cx, did)) {
            Some(path) if rb_sys_function(&path).is_some_and(may_raise) => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    })
    .is_some();
    if !raises {
        span_lint_and_help(
            cx,
            UNRAISING_CALL_IN_RAW,
            expr.span.source_callsite(),
            "a protected call of nothing that can raise",
            None,
            "a C call that cannot raise needs no rb_protect; if this one can, it is missing \
             from makiri_lints' table the other way (NON_RAISING lists it)",
        );
    }
}

/// Whether `f`, a call's callee, is `raw`'s `call`.
fn is_call(cx: &LateContext<'_>, f: &Expr<'_>) -> bool {
    let ExprKind::Path(ref qpath) = f.kind else {
        return false;
    };
    let Res::Def(_, did) = cx.qpath_res(qpath, f.hir_id) else {
        return false;
    };
    CALL.contains(&path_of(cx, did).as_str())
}

/// Whether the expression at `hir_id` is inside the closure handed to `call`.
fn inside_call(cx: &LateContext<'_>, hir_id: HirId) -> bool {
    let mut child = hir_id;
    for (id, node) in cx.tcx.hir_parent_iter(hir_id) {
        match node {
            Node::Expr(e) => {
                if let ExprKind::Call(f, args) = e.kind
                    && args.iter().any(|a| a.hir_id == child)
                    && is_call(cx, f)
                {
                    return true;
                }
            }
            Node::Item(_) => return false,
            _ => {}
        }
        child = id;
    }
    false
}
