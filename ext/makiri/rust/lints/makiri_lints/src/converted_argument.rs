//! `converted_argument`: a registered method whose argument magnus converts
//! with the caller's Ruby.
//!
//! `method!(f, n)` and `function!(f, n)` expand to `f.call_handle_error(..)`,
//! a method of one of `magnus::method`'s traits (`Method2`, `RubyFunction1`,
//! `MethodCAry`, ...), implemented for every `Fn` whose parameters are
//! `TryConvert`. The trait's type arguments at that call are `Self` (the
//! function), each converted parameter in order (the receiver first for a
//! method), and the return type last; so the converted types are read off the
//! call itself, whatever `f` is (a path, a closure, a generic instance). A
//! parameter magnus does not convert - `&Ruby`, an arity -1 `&[Value]` - is
//! not a type argument and so is never looked at.
//!
//! Accepted: `Value`; `bool` (truthiness, read from the value's bits); a type of the crate being checked (its `TryConvert` is
//! Makiri's own, e.g. `HtmlSelf`); a reference to a `TypedData` type or to
//! one of the crate's types (magnus checks the class, calling no Ruby); and
//! `Option` of any of these. Everything else - magnus's `RString`, `RArray`,
//! `RHash`, `Integer`, `Float`, `Symbol`, std's integers and floats, `String`,
//! `Vec`, tuples - is reported at the registered function's name, the one
//! span of the expansion the user wrote.

use crate::{CONVERTED_ARGUMENT, typed_data};
use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;
use rustc_middle::ty::{self, Ty};

pub(crate) fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let ExprKind::MethodCall(segment, receiver, _, _) = expr.kind else {
        return;
    };
    if segment.ident.name.as_str() != "call_handle_error" {
        return;
    }
    let typeck = cx.typeck_results();
    let Some(did) = typeck.type_dependent_def_id(expr.hir_id) else {
        return;
    };
    let Some(trait_did) = cx.tcx.trait_of_assoc(did) else {
        return;
    };
    if !cx
        .tcx
        .def_path_str(trait_did)
        .starts_with("magnus::method::")
    {
        return;
    }
    let tys: Vec<Ty<'tcx>> = typeck.node_args(expr.hir_id).types().collect();
    let [_, converted @ .., _] = tys.as_slice() else {
        return;
    };
    for &ty in converted {
        if !accepted(cx, ty) {
            span_lint_and_help(
                cx,
                CONVERTED_ARGUMENT,
                receiver.span,
                format!(
                    "this registered function takes `{ty}`, which magnus converts \
                     with the caller's Ruby"
                ),
                None,
                "take a `Value` and read it with `string_arg` / `name_arg` / `integer_arg`",
            );
        }
    }
}

fn accepted<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> bool {
    match ty.kind() {
        ty::Bool => true,
        ty::Adt(adt, args) => {
            let did = adt.did();
            if did.is_local() {
                return true;
            }
            let krate = cx.tcx.crate_name(did.krate);
            let name = cx.tcx.item_name(did);
            match (krate.as_str(), name.as_str()) {
                ("magnus", "Value") => true,
                ("core", "Option") => args.types().next().is_some_and(|t| accepted(cx, t)),
                _ => false,
            }
        }
        ty::Ref(_, inner, _) => match inner.kind() {
            ty::Adt(adt, _) if adt.did().is_local() => true,
            _ => typed_data(cx)
                .is_some_and(|t| clippy_utils::ty::implements_trait(cx, *inner, t, &[])),
        },
        _ => false,
    }
}
