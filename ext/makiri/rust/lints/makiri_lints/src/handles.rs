//! What keeps a wrapped object's data borrowed safely, beyond the types.
//!
//! A payload borrowed as `&T` keeps its object alive to the type checker
//! only: the conservative stack scan does not take the data pointer for the
//! object. `typed::Typed` holds the object (`rb::kept::Kept`) until after its
//! last borrow - so long as the handle is dropped, not forgotten. And
//! `TypedType::get` / `get_known`, which borrow from a bare `Value` and hold
//! nothing, are sound only for a value Ruby keeps for the whole borrow (a
//! receiver, an argument, a slot's value).
//!
//! - `forgotten_handle`: a `Typed` or `Kept` given to `mem::forget` (or
//!   wrapped in `ManuallyDrop`), whose `Drop` - the one that keeps the object
//!   where the scan sees it - then never runs.
//! - `unheld_payload_borrow`: a call of `TypedType::get` / `get_known`, each
//!   one allowed where it is made with the reason its value is kept.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::Expr;
use rustc_lint::LateContext;
use rustc_middle::ty::{self, Ty};

use crate::paths::{callee, is_extension, path_of};
use crate::{FORGOTTEN_HANDLE, UNHELD_PAYLOAD_BORROW};

/// The handles whose `Drop` keeps an object alive, by their paths.
const HANDLES: &[&str] = &["nupur::typed::Typed", "nupur::rb::kept::Kept"];

/// `TypedType::get` / `get_known`, as `path_of` spells them: an inherent
/// `impl` block is an unnamed part of the path.
const BARE_BORROWS: &[&str] = &["nupur::typed::get", "nupur::typed::get_known"];

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
    if BARE_BORROWS.contains(&path.as_str()) {
        span_lint_and_help(
            cx,
            UNHELD_PAYLOAD_BORROW,
            expr.span.source_callsite(),
            format!("`{path}` borrows a payload through a bare `Value`, which keeps nothing"),
            None,
            "borrow through a `Typed` handle (`TypedType::check`, `wrap`), which holds the \
             object; or, for a value Ruby keeps for the whole borrow (a receiver, an \
             argument, a slot's value), allow this call with that reason",
        );
        return;
    }
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
