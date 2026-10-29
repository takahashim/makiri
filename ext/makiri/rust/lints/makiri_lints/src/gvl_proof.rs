//! `ffi_without_gvl_proof`: a call into Ruby's C API from a function that
//! holds no proof it runs where Ruby may be called - a Ruby thread, with the
//! GVL.
//!
//! Ruby's C functions touch the VM: called from a thread Ruby does not run,
//! or with the GVL released, they read a null execution context or race the
//! interpreter. The layer's rule is that a safe function shows the proof in
//! its signature: a `&Ruby` (the token only an entry makes), or a Ruby value
//! (`ReprValue`: a value is `!Send` and made only where a token is), or a
//! handle of the layer's that is made only from one of those (`Encoding`,
//! `Typed`, `Unwritten`), or such a `self`. So a function whose
//! body calls `rb_sys::` must take one of them - checked on every function
//! of the crate, private ones included, so a public function cannot lend its
//! proof to a helper that has none: the helper needs its own. A call inside a
//! closure counts against the function the closure is in.
//!
//! Exempt by construction: an `extern "C"` function, which is an entry Ruby
//! itself calls (a GC callback, `rb_protect`'s thunk, a trampoline), and an
//! `unsafe fn`, whose contract states the GVL and which safe code cannot
//! call. Any other site that has a reason is an `allow` with it.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::Expr;
use rustc_hir::def_id::DefId;
use rustc_lint::LateContext;
use rustc_middle::ty::{self, Ty};

use crate::FFI_WITHOUT_GVL_PROOF;
use crate::boundaries::{NUPUR, callee, path_of};

/// The layer's handles that exist only where a token or a Ruby value did.
const PROOF_TYPES: &[&str] = &["Ruby", "Encoding", "Typed", "Unwritten"];

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE).as_str() != NUPUR {
        return;
    }
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let target = path_of(cx, did);
    if !target.starts_with("rb_sys::") {
        return;
    }
    let owner = cx.tcx.hir_enclosing_body_owner(expr.hir_id);
    let root = cx.tcx.typeck_root_def_id(owner.to_def_id());
    if !matches!(
        cx.tcx.def_kind(root),
        rustc_hir::def::DefKind::Fn | rustc_hir::def::DefKind::AssocFn
    ) {
        return; /* a const or static initialiser: run at build time */
    }
    let sig = cx.tcx.fn_sig(root).instantiate_identity().skip_binder();
    if !sig.abi().is_rustic_abi() {
        return; /* an entry Ruby calls */
    }
    if sig.safety().is_unsafe() {
        return; /* the caller's contract, which safe code cannot call into */
    }
    if sig.inputs().iter().any(|&t| proves(cx, t)) {
        return;
    }
    span_lint_and_help(
        cx,
        FFI_WITHOUT_GVL_PROOF,
        expr.span.source_callsite(),
        format!("`{target}`, Ruby's C API, from a function that holds no proof of the GVL"),
        None,
        "take `&Ruby` (or a Ruby value, or a handle made from one): a function callable \
         with nothing could be called from a thread Ruby does not run",
    );
}

/// Whether an argument of type `ty` is a proof: a `&Ruby`, a Ruby value, a
/// handle of the layer's made only from one, behind any references.
fn proves<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> bool {
    let ty = ty.peel_refs();
    if let ty::Adt(def, _) = ty.kind()
        && is_proof_type(cx, def.did())
    {
        return true;
    }
    let (Some(repr), _) = crate::traits(cx) else {
        return false;
    };
    clippy_utils::ty::implements_trait(cx, ty, repr, &[])
}

fn is_proof_type(cx: &LateContext<'_>, did: DefId) -> bool {
    crate::ours(cx, did) && PROOF_TYPES.contains(&cx.tcx.item_name(did).as_str())
}
