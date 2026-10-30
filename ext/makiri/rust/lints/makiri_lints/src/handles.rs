//! What keeps a wrapped object's data borrowed safely, beyond the types.
//!
//! A payload borrowed as `&T` keeps its object alive to the type checker
//! only: the conservative stack scan does not take the data pointer for the
//! object. `typed::Typed` holds the object (`rb::kept::Kept`) until after its
//! last borrow - so long as the handle is dropped, not forgotten. A handle
//! is the only way to a payload (there is no `&Value -> &T`), so that is
//! the one thing left to check:
//!
//! - `forgotten_handle`: a `Typed` or `Kept` given to `mem::forget` (or
//!   wrapped in `ManuallyDrop`), whose `Drop` - the one that keeps the object
//!   where the scan sees it - then never runs.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::Expr;
use rustc_lint::LateContext;
use rustc_middle::ty::{self, Ty};

use crate::FORGOTTEN_HANDLE;
use crate::paths::{callee, is_extension, path_of};

/// The handles whose `Drop` keeps an object alive, by their paths.
const HANDLES: &[&str] = &["nupur::typed::Typed", "nupur::rb::kept::Kept"];

/// What leaks a value without running its `Drop`.
const FORGETS: &[&str] = &["core::mem::forget", "core::mem::manually_drop::new"];

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let path = path_of(cx, did);
    if FORGETS.contains(&path.as_str())
        && let rustc_hir::ExprKind::Call(_, [arg]) = expr.kind
        && holds_handle(cx, cx.typeck_results().expr_ty(arg))
    {
        span_lint_and_help(
            cx,
            FORGOTTEN_HANDLE,
            expr.span.source_callsite(),
            "a `Typed` / `Kept` handle forgotten: its `Drop` keeps the object alive",
            None,
            "let the handle drop after the last borrow of its data; forgotten, the \
             object may be collected while the data is still read",
        );
    }
}

/// Whether `ty` is one of the handles, behind any references.
fn holds_handle<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> bool {
    match ty.peel_refs().kind() {
        ty::Adt(def, _) => HANDLES.contains(&path_of(cx, def.did()).as_str()),
        _ => false,
    }
}
