//! `gvl_token_outside_entry`: the GVL's token (`rb::Ruby`) made anywhere but
//! the entries Ruby calls.
//!
//! A `Ruby` is the proof that the thread holds the GVL, and every function
//! that calls Ruby takes one (`ffi_without_gvl_proof`). It is made by
//! `Ruby::get_unchecked`, whose contract is that very fact, and handed down
//! from the two entries Ruby itself calls with the GVL held: a method's
//! trampoline (`rb::method::finish`, `finish_cary`) and the extension's
//! `Init_` (`rb::method::run_init`). The four are `unsafe fn`, so a call
//! needs an `unsafe` block - but a file that may hold one could still make a
//! token where no GVL is held, and only a count of its `unsafe` blocks would
//! change. So each is held to its one place:
//!
//! - `get_unchecked`: in `nupur::rb::method`, where the entries are;
//! - `finish` / `finish_cary`: in the expansion of nupur's `method!` /
//!   `function!` (`__trampoline!`), the trampolines Ruby is handed (and
//!   `finish` in `finish_cary`, beside it);
//! - `run_init`: in an `extern "C"` function named `Init_*`, which Ruby
//!   calls as the extension loads.
//!
//! `nupur_sys`'s own token, which its reads that read no value take, is held
//! the same way, to the functions named in `GVL_ASSUMERS`: Nupur's
//! `Ruby::get_unchecked`, and the GC's free callbacks.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::Expr;
use rustc_lint::LateContext;
use rustc_span::Span;

use crate::GVL_TOKEN_OUTSIDE_ENTRY;
use crate::paths::{callee, is_extension, module_of, path_of};

/// `Ruby::get_unchecked`, as `path_of` spells it: an inherent `impl` block
/// is an unnamed part of the path.
const GET_UNCHECKED: &str = "nupur::rb::ruby::get_unchecked";
const TRAMPOLINES: &[&str] = &[
    "nupur::rb::method::finish",
    "nupur::rb::method::finish_cary",
];
const RUN_INIT: &str = "nupur::rb::method::run_init";
/// `nupur_sys`'s GVL token, made on the caller's word, and the functions
/// that may make it - the maker of Nupur's own token, `Ruby::get_unchecked`
/// (itself held to `rb::method` above), and the GC's free callbacks, entries
/// Ruby calls with the GVL held, which make it once and hand it down.
const GVL_ASSUME: &str = "nupur_sys::value::get_unchecked";
const GVL_ASSUMERS: &[&str] = &[
    "nupur::rb::ruby::get_unchecked",
    "nupur::typed::callbacks::free_cb",
    "nupur::typed::callbacks::free_embedded_cb",
];
/// The macros of nupur's whose expansion is a trampoline, by name: a
/// `#[macro_export]` macro is named at the crate's root but defined where it
/// is written, so it is matched by its crate and its name.
const TRAMPOLINE_MACROS: &[&str] = &["method", "function", "__trampoline"];

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let target = path_of(cx, did);
    let allowed = if target == GET_UNCHECKED {
        module_of(cx, expr.hir_id) == "nupur::rb::method"
    } else if TRAMPOLINES.contains(&target.as_str()) {
        /* `finish_cary` is `finish` for an arity -1 method, beside it. */
        from_trampoline_macro(cx, expr.span) || module_of(cx, expr.hir_id) == "nupur::rb::method"
    } else if target == RUN_INIT {
        in_init_function(cx, expr)
    } else if target == GVL_ASSUME {
        GVL_ASSUMERS.contains(&enclosing_fn(cx, expr).as_str())
    } else {
        return;
    };
    if allowed {
        return;
    }
    span_lint_and_help(
        cx,
        GVL_TOKEN_OUTSIDE_ENTRY,
        expr.span.source_callsite(),
        format!("`{target}` outside the entry Ruby calls it from"),
        None,
        "a `Ruby` token is made only where Ruby calls in with the GVL held: \
         `get_unchecked` in `rb::method`, `finish` in a `method!` / `function!` \
         trampoline, `run_init` in the extension's `extern \"C\" fn Init_*`; and \
         `nupur_sys`'s token only where Nupur has one to show (GVL_ASSUMERS)",
    );
}

/// Whether `span` comes from the expansion of one of nupur's trampoline
/// macros, at any depth.
fn from_trampoline_macro(cx: &LateContext<'_>, span: Span) -> bool {
    span.macro_backtrace().any(|expn| {
        expn.macro_def_id.is_some_and(|m| {
            path_of(cx, m).starts_with("nupur::")
                && TRAMPOLINE_MACROS.contains(&cx.tcx.item_name(m).as_str())
        })
    })
}

/// Whether `expr` is in the body of an `extern "C"` function named `Init_*`.
fn in_init_function(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    let owner = cx.tcx.hir_enclosing_body_owner(expr.hir_id).to_def_id();
    let root = cx.tcx.typeck_root_def_id(owner);
    matches!(cx.tcx.def_kind(root), rustc_hir::def::DefKind::Fn)
        && cx.tcx.item_name(root).as_str().starts_with("Init_")
        && !cx
            .tcx
            .fn_sig(root)
            .instantiate_identity()
            .skip_binder()
            .abi()
            .is_rustic_abi()
}

/// The path of the function `expr` is in - the one a closure is written in,
/// not the closure.
fn enclosing_fn(cx: &LateContext<'_>, expr: &Expr<'_>) -> String {
    let owner = cx.tcx.hir_enclosing_body_owner(expr.hir_id).to_def_id();
    path_of(cx, cx.tcx.typeck_root_def_id(owner))
}
