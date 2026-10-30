//! `discarded_ruby_error`: a `Result<_, bridge::rb::Error>` whose `Err` is
//! thrown away - `.ok()`, `.unwrap_or*`, `.is_ok()`, `.or(..)`, a
//! `.map_err` / `.or_else` closure that ignores it, `let _ =` or
//! `let _name =` (an `Error` itself too), a pattern
//! that leaves it unbound (`if let Ok(..)`, `let Ok(..) else`, a `match`
//! arm `Err(_)` or `_`), or an `Error` handed to `drop` / `mem::forget`: a
//! raise or a `throw` in it would be lost, and the call taken as having
//! answered. An `Err` becomes "no answer" only through `Error::rescue`,
//! which names what it handles and hands every exit back; the extension
//! `forbid`s the lint, so no `allow` reopens it there.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::{Expr, ExprKind, LetStmt, PatKind, QPath};
use rustc_lint::LateContext;
use rustc_span::Span;

use crate::DISCARDED_RUBY_ERROR;
use crate::paths::{callee, is_extension, path_of};

/// The methods that answer a `Result` without its error.
const DISCARDING: &[&str] = &[
    "ok",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "map_or",
    "map_or_else",
    "is_ok",
    "is_err",
    "is_ok_and",
    "is_err_and",
    "or",
];

/// The methods whose closure is handed the `Err`, which it may ignore.
const HANDING_ERR: &[&str] = &["map_err", "or_else"];

/// Whether `ty` is `Result<_, nupur::rb::error::Error>`.
fn is_ruby_result<'tcx>(cx: &LateContext<'tcx>, ty: rustc_middle::ty::Ty<'tcx>) -> bool {
    let rustc_middle::ty::Adt(adt, args) = ty.kind() else {
        return false;
    };
    if path_of(cx, adt.did()) != "core::result::Result" {
        return false;
    }
    args.types().nth(1).is_some_and(|e| is_ruby_error(cx, e))
}

/// Whether `ty` is `nupur::rb::error::Error` itself.
fn is_ruby_error<'tcx>(cx: &LateContext<'tcx>, ty: rustc_middle::ty::Ty<'tcx>) -> bool {
    ty.ty_adt_def()
        .is_some_and(|e| path_of(cx, e.did()) == "nupur::rb::error::Error")
}

/// Whether `pat` binds nothing a reader would use: `_`, or a name that
/// starts with one.
fn ignores(pat: &rustc_hir::Pat<'_>) -> bool {
    match pat.kind {
        PatKind::Wild => true,
        PatKind::Binding(_, _, ident, None) => ident.name.as_str().starts_with('_'),
        _ => false,
    }
}

/// Whether `pat`, on a `Result`, leaves its `Err` unbound: `Ok(..)` (the
/// `Err` falls through), `Err(_)`, or `_`.
fn leaves_err(pat: &rustc_hir::Pat<'_>) -> bool {
    match pat.kind {
        PatKind::Wild => true,
        PatKind::TupleStruct(QPath::Resolved(_, path), fields, _) => {
            match path.segments.last().map(|s| s.ident.name.as_str()) {
                Some("Ok") => true,
                Some("Err") => fields.iter().all(ignores),
                _ => false,
            }
        }
        _ => false,
    }
}

fn report_discarded(cx: &LateContext<'_>, span: Span, how: &str) {
    span_lint_and_help(
        cx,
        DISCARDED_RUBY_ERROR,
        span,
        format!("a Ruby error is thrown away ({how})"),
        None,
        "an Err of bridge::rb::Error is a raise, a throw or a refusal: pass it on with `?`; \
         where dropping it is the answer, allow this with the reason",
    );
}

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if let ExprKind::MethodCall(seg, receiver, ..) = expr.kind
        && DISCARDING.contains(&seg.ident.name.as_str())
        && is_ruby_result(cx, cx.typeck_results().expr_ty(receiver))
    {
        report_discarded(
            cx,
            expr.span.source_callsite(),
            &format!(".{}()", seg.ident.name),
        );
    }
    /* `.map_err(|_| ..)` / `.or_else(|_| ..)`: the closure is handed the
     * `Err` and ignores it. */
    if let ExprKind::MethodCall(seg, receiver, [arg], _) = expr.kind
        && HANDING_ERR.contains(&seg.ident.name.as_str())
        && let ExprKind::Closure(closure) = arg.kind
        && cx
            .tcx
            .hir_body(closure.body)
            .params
            .first()
            .is_some_and(|p| ignores(p.pat))
        && is_ruby_result(cx, cx.typeck_results().expr_ty(receiver))
    {
        report_discarded(
            cx,
            expr.span.source_callsite(),
            &format!(".{}(|_| ..)", seg.ident.name),
        );
    }
    /* `if let Ok(..) = r` / `while let Ok(..) = r` / `if let Err(_) = r`:
     * the `Err` is left unbound. */
    if let ExprKind::Let(let_expr) = expr.kind
        && leaves_err(let_expr.pat)
        && is_ruby_result(cx, cx.typeck_results().expr_ty(let_expr.init))
    {
        report_discarded(cx, expr.span.source_callsite(), "if let");
    }
    /* A `match` arm `Err(_)` or `_`, on a Ruby result. `Ok(..)` arms are
     * fine here: a `match` must cover the `Err` too. */
    if let ExprKind::Match(scrutinee, arms, _) = expr.kind
        && is_ruby_result(cx, cx.typeck_results().expr_ty(scrutinee))
    {
        for arm in arms {
            let unbound = match arm.pat.kind {
                PatKind::Wild => true,
                PatKind::TupleStruct(..) => {
                    !matches!(arm.pat.kind, PatKind::TupleStruct(QPath::Resolved(_, path), ..)
                        if path.segments.last().is_some_and(|s| s.ident.name.as_str() == "Ok"))
                        && leaves_err(arm.pat)
                }
                _ => false,
            };
            if unbound {
                report_discarded(cx, arm.pat.span.source_callsite(), "a match arm Err(_)");
            }
        }
    }
    /* `drop(e)` / `mem::forget(e)` of an `Error` or a Ruby result. */
    if let ExprKind::Call(_, [arg]) = expr.kind
        && let Some(did) = callee(cx, expr)
        && matches!(
            path_of(cx, did).as_str(),
            "core::mem::drop" | "core::mem::forget"
        )
        && {
            let ty = cx.typeck_results().expr_ty(arg);
            is_ruby_error(cx, ty) || is_ruby_result(cx, ty)
        }
    {
        report_discarded(cx, expr.span.source_callsite(), "drop / mem::forget");
    }
}

pub fn check_local<'tcx>(cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    /* `let _ = r` and `let _r = r`: a name that starts with `_` says it is
     * not read, and an `Error` bound so is as lost as a `Result`'s. */
    if ignores(local.pat)
        && let Some(init) = local.init
        && {
            let ty = cx.typeck_results().expr_ty(init);
            is_ruby_result(cx, ty) || is_ruby_error(cx, ty)
        }
    {
        report_discarded(cx, local.span.source_callsite(), "let _ =");
    }
    if local.els.is_some()
        && leaves_err(local.pat)
        && let Some(init) = local.init
        && is_ruby_result(cx, cx.typeck_results().expr_ty(init))
    {
        report_discarded(cx, local.span.source_callsite(), "let Ok(..) else");
    }
}
