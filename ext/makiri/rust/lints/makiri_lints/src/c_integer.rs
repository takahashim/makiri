//! `c_integer_outside_int`: a C integer handled outside the one module that
//! converts them, `nupur::int`.
//!
//! A C `long` is 64 bits where Makiri is built and 32 bits on Windows (LLP64),
//! and `c_long` is a type alias: on the machines the code is written and
//! linted on it IS `i64`, so `n as c_long` is a cast from `i64` to `i64`, and
//! neither rustc nor clippy can see that it truncates elsewhere. Ruby's own
//! integer ranges follow the `long` (a Fixnum is `LONG_MIN / 2 ..= LONG_MAX /
//! 2`), so every conversion between Rust's widths and Ruby's is one place to
//! get Windows wrong - and those places were found one review at a time.
//!
//! So the conversions live in one module, written once and tested at both
//! widths, and outside it the Ruby boundary - the binding layer and the
//! extension's `bridge`, `glue` and `init` - may neither
//!
//! - write an integer-to-integer `as` cast (the source's type may be an
//!   alias the lint cannot see either - `rb_sys`'s answers are `c_long`), nor
//! - name `c_long` / `c_ulong`, so no signature carries one: a length is a
//!   `usize`, an index an `i64`, and `nupur::int` says how each becomes C's.
//!
//! A conversion with `From` / `TryFrom` needs no exemption: it compiles to
//! what each target allows, and fails where a value does not fit.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind, HirId, Path};
use rustc_lint::LateContext;
use rustc_middle::ty;

use crate::C_INTEGER_OUTSIDE_INT;
use crate::paths::{NUPUR, module_of, ours, path_of, within};

/// Whether code in `module` is held to the rule: the binding layer and the
/// extension's Ruby boundary, bar the module that converts. The self-tests'
/// probes are in both crates.
fn held(module: &str) -> bool {
    if !ours(module) || within(module, "nupur::int") {
        return false;
    }
    [NUPUR, "bridge", "glue", "init", "lint_probe"]
        .iter()
        .any(|m| within(module, m))
}

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let ExprKind::Cast(from, _) = expr.kind else {
        return;
    };
    if expr.span.in_external_macro(cx.tcx.sess.source_map()) {
        return;
    }
    let typeck = cx.typeck_results();
    let (src, dst) = (typeck.expr_ty(from), typeck.expr_ty(expr));
    if !(is_integer(src) && is_integer(dst)) {
        return;
    }
    if !held(&module_of(cx, expr.hir_id)) {
        return;
    }
    span_lint_and_help(
        cx,
        C_INTEGER_OUTSIDE_INT,
        expr.span.source_callsite(),
        format!("an `as` cast from `{src}` to `{dst}` at the Ruby boundary"),
        None,
        "convert with `From` / `TryFrom`, or through `nupur::int`: a C `long` is 32 bits on \
         Windows, and an `as` there truncates where this machine cannot show it",
    );
}

pub fn check_path(cx: &LateContext<'_>, path: &Path<'_>, hir_id: HirId) {
    let Res::Def(DefKind::TyAlias, did) = path.res else {
        return;
    };
    let target = path_of(cx, did);
    let krate = target.split("::").next().unwrap_or("");
    let name = cx.tcx.item_name(did);
    if !matches!(krate, "core" | "std") || !matches!(name.as_str(), "c_long" | "c_ulong") {
        return;
    }
    if path.span.in_external_macro(cx.tcx.sess.source_map()) || !held(&module_of(cx, hir_id)) {
        return;
    }
    span_lint_and_help(
        cx,
        C_INTEGER_OUTSIDE_INT,
        path.span.source_callsite(),
        format!("`{name}` named at the Ruby boundary"),
        None,
        "take and return Rust's widths (`usize`, `i64`) and let `nupur::int` make the C `long`: \
         it is 32 bits on Windows",
    );
}

fn is_integer(t: ty::Ty<'_>) -> bool {
    matches!(t.kind(), ty::Int(_) | ty::Uint(_))
}
