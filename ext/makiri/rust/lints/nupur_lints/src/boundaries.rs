//! The layer's boundaries, checked on the compiled code rather than its
//! text: every path here is the RESOLVED definition, so a `use` alias, a
//! re-export or a macro expansion cannot hide a reference, and a comment or
//! a string cannot fake one. (`script/check_unsafe_boundaries.rb` checked
//! such lines with regular expressions, and one of them - `RUBY_LAYER` -
//! held a backspace for months and matched nothing.)
//!
//! - `ruby_in_ruby_free_module`: the layer's Ruby-free module names nothing
//!   of the Ruby layer.
//! - `ruby_in_gvl_released_body`: a GVL-released body calls nothing that
//!   could reach Ruby.
//! - `runs_callers_ruby`: a call that would run the caller's Ruby code, each
//!   one allowed where it is made, with its reason.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::Res;
use rustc_hir::def_id::DefId;
use rustc_hir::{Expr, ExprKind, HirId, Path, QPath};
use rustc_lint::LateContext;
use rustc_span::Span;

use crate::paths::{
    BYTES_LENDING_METHODS, CALLERS_RUBY_RUNNERS, GVL_RELEASED_CALLS, callee, in_ruby_free_crate,
    is_extension, module_of, ours, path_of, within,
};
use crate::{RUBY_IN_GVL_RELEASED_BODY, RUBY_IN_RUBY_FREE_MODULE, RUNS_CALLERS_RUBY};

/// The layer's Ruby-free modules: the checked copy, which Miri runs without
/// a Ruby and which sits beside the Ruby layer in its crate.
/// `lint_probe::ruby_free` stands in for one in the layer's self-test.
const RUBY_FREE_MODULES: &[&str] = &["nupur::utf8_copy", "nupur::lint_probe::ruby_free"];

fn in_ruby_free_module(path: &str) -> bool {
    RUBY_FREE_MODULES.iter().any(|m| within(path, m))
}

/// Whether code at `path` is known Ruby-free, so an edit's closure or a
/// GVL-released body may call it: the layer's Ruby-free modules and the
/// extension's Ruby-free crates.
fn ruby_free(path: &str) -> bool {
    in_ruby_free_module(path) || in_ruby_free_crate(path)
}

/// What a Ruby-free module may not name: the layer, the extension, and
/// Ruby's C API.
fn ruby_layer(path: &str) -> bool {
    path.starts_with("rb_sys::") || (ours(path) && !ruby_free(path))
}

/* ---- ruby_in_ruby_free_module ---- */

pub fn check_path(cx: &LateContext<'_>, path: &Path<'_>, hir_id: HirId) {
    if !is_extension(cx) {
        return;
    }
    let Res::Def(_, did) = path.res else {
        return;
    };
    report_ruby_layer(cx, did, path.span.source_callsite(), hir_id);
}

fn report_ruby_layer(cx: &LateContext<'_>, did: DefId, span: Span, hir_id: HirId) {
    if !in_ruby_free_module(&module_of(cx, hir_id)) {
        return;
    }
    let target = path_of(cx, did);
    if ruby_layer(&target) {
        span_lint_and_help(
            cx,
            RUBY_IN_RUBY_FREE_MODULE,
            span,
            format!("a Ruby-free module names `{target}`, of the Ruby layer"),
            None,
            "the module runs without a Ruby (Miri runs it) and in a GVL-released body; \
             move the Ruby half out of it",
        );
    }
}

/* ---- the expression checks ---- */

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    if let ExprKind::MethodCall(..) = expr.kind
        && let Some(did) = callee(cx, expr)
    {
        report_ruby_layer(cx, did, expr.span.source_callsite(), expr.hir_id);
    }
    check_layer_calls(cx, expr);
}

/// The layer's guarantees, in any extension on Nupur and in the layer
/// itself: what runs the caller's Ruby, and what a closure the layer runs
/// with the GVL released (or with a String's bytes lent) may call.
fn check_layer_calls<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let path = path_of(cx, did);

    if runs_callers_ruby(&path) {
        span_lint_and_help(
            cx,
            RUNS_CALLERS_RUBY,
            expr.span.source_callsite(),
            format!("`{path}` runs the caller's Ruby code"),
            None,
            "an extension on Nupur runs none of its caller's Ruby: no argument is converted, \
             no method of a caller's object called; a call that must is allowed where it is \
             made, with its reason, holding no guard or copy across it",
        );
    }

    /* The body is the last argument (`without_gvl(ruby, body)`): matched
     * as `[body]` alone, this saw nothing once the token joined it. */
    if GVL_RELEASED_CALLS.contains(&path.as_str())
        && let ExprKind::Call(_, [.., body]) = expr.kind
    {
        check_gvl_released(cx, body, &path);
    }
    if BYTES_LENDING_METHODS.contains(&path.as_str())
        && let ExprKind::MethodCall(_, _, [body], _) = expr.kind
    {
        check_gvl_released(cx, body, &path);
    }
}

/// The calls that run the caller's Ruby code: Ruby's C API that converts or
/// calls (`to_s`, `to_str`, a method, a block) - rb-sys's own, or
/// `nupur_sys::raising`'s wrapper of it, named the same - and the layer's
/// functions over it ([`CALLERS_RUBY_RUNNERS`]).
fn runs_callers_ruby(path: &str) -> bool {
    if let Some(name) = path
        .strip_prefix("rb_sys::")
        .or_else(|| path.strip_prefix("nupur_sys::raising::"))
        .and_then(|p| p.rsplit("::").next())
    {
        return matches!(
            name,
            "rb_String"
                | "rb_obj_as_string"
                | "rb_respond_to"
                | "rb_block_call"
                | "rb_call_super"
                | "rb_str_to_str"
                | "rb_check_string_type"
                | "rb_convert_type"
                | "rb_check_convert_type"
        ) || [
            "rb_funcall",
            "rb_yield",
            "rb_eval_string",
            "rb_string_value",
        ]
        .iter()
        .any(|p| name.starts_with(p));
    }
    CALLERS_RUBY_RUNNERS.contains(&path)
}

/// A GVL-released body: everything it calls must be Ruby-free code
/// ([`ruby_free`]) or a library's - never the layer, the extension, Ruby's C
/// API, or a function value whose target no one can see.
fn check_gvl_released<'tcx>(cx: &LateContext<'tcx>, body: &'tcx Expr<'tcx>, holder: &str) {
    let ExprKind::Closure(closure) = body.kind else {
        span_lint_and_help(
            cx,
            RUBY_IN_GVL_RELEASED_BODY,
            body.span,
            format!("a GVL-released body (`{holder}`'s) that is not a closure written here"),
            None,
            "pass a closure, so what it calls can be checked",
        );
        return;
    };
    let inner = cx.tcx.hir_body(closure.body);
    let _ = clippy_utils::visitors::for_each_expr(cx.tcx, inner.value, |e| {
        if matches!(e.kind, ExprKind::Call(..) | ExprKind::MethodCall(..)) {
            let bad = match callee(cx, e) {
                Some(did) => {
                    let path = path_of(cx, did);
                    path.starts_with("rb_sys::") || (ours(&path) && !ruby_free(&path))
                }
                /* A call through a local function value or a pointer. */
                None => {
                    matches!(e.kind, ExprKind::Call(f, _) if !matches!(f.kind, ExprKind::Path(QPath::TypeRelative(..))))
                }
            };
            if bad {
                span_lint_and_help(
                    cx,
                    RUBY_IN_GVL_RELEASED_BODY,
                    e.span.source_callsite(),
                    format!("a GVL-released body (`{holder}`'s) calls what is not known Ruby-free"),
                    None,
                    "with the GVL released no Ruby may run, and only a library or code \
                     declared Ruby-free is known not to; hoist this call out of the closure",
                );
            }
        }
        core::ops::ControlFlow::<()>::Continue(())
    });
}
