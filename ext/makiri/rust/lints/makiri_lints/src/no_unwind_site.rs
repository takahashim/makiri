//! `unguarded_no_unwind_site`: every `extern "C"` function and every `Drop`
//! runs its body under `no_unwind::must_not_unwind` - an unwind out of
//! either aborts the process, and `rake no_unwind` proves each such body
//! unwind-free only where it is a site.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::Body;
use rustc_hir::intravisit::FnKind;
use rustc_lint::LateContext;
use rustc_span::Span;
use rustc_span::def_id::LocalDefId;

use crate::UNGUARDED_NO_UNWIND_SITE;
use crate::paths::{callee, is_extension, path_of};

/// The calls that put a body under `must_not_unwind`: it, and the
/// trampolines' and `Init_makiri`'s helpers, which do.
const GUARDS: &[&str] = &[
    "makiri_engine::no_unwind::must_not_unwind",
    "nupur_sys::no_unwind::must_not_unwind",
    "nupur::rb::method::finish",
    "nupur::rb::method::finish_cary",
    "nupur::rb::method::run_init",
];

pub fn check_fn<'tcx>(
    cx: &LateContext<'tcx>,
    kind: FnKind<'tcx>,
    body: &'tcx Body<'tcx>,
    span: Span,
    did: LocalDefId,
) {
    if !is_extension(cx) {
        return;
    }
    let site = match kind {
        FnKind::ItemFn(_, _, header) if !header.abi.is_rustic_abi() => "an `extern \"C\"` function",
        FnKind::Method(_, sig) if !sig.header.abi.is_rustic_abi() => "an `extern \"C\"` function",
        FnKind::Method(..) if is_drop(cx, did) => "a `Drop`",
        _ => return,
    };
    let guarded =
        clippy_utils::visitors::for_each_expr(cx.tcx, body.value, |e| match callee(cx, e) {
            Some(d) if GUARDS.contains(&path_of(cx, d).as_str()) => {
                core::ops::ControlFlow::Break(())
            }
            _ => core::ops::ControlFlow::Continue(()),
        })
        .is_some();
    if !guarded {
        span_lint_and_help(
            cx,
            UNGUARDED_NO_UNWIND_SITE,
            span,
            format!("{site} whose body does not run under `must_not_unwind`"),
            None,
            "an unwind out of it aborts the process; add a site to src/no_unwind.rs \
             and run the body through must_not_unwind, so `rake no_unwind` proves it",
        );
    }
}

fn is_drop(cx: &LateContext<'_>, did: LocalDefId) -> bool {
    let Some(impl_did) = cx
        .tcx
        .inherent_impl_of_assoc(did.to_def_id())
        .or_else(|| cx.tcx.trait_impl_of_assoc(did.to_def_id()))
    else {
        return false;
    };
    cx.tcx
        .impl_opt_trait_ref(impl_did)
        .is_some_and(|t| Some(t.skip_binder().def_id) == cx.tcx.lang_items().drop_trait())
}
