//! `ruby_value_on_heap`: a Ruby value (a `bridge::rb::ReprValue` type) kept where
//! Ruby's GC cannot see it.
//!
//! Ruby's GC finds a value by scanning the machine stack. One in a `Vec`, a
//! `Box`, a `HashMap` or any other heap allocation, in a `static`, or in the
//! data of a wrapped object (a `Payload` type, which lives in Ruby's heap) is
//! invisible to that scan: the object can be collected, or moved by
//! compaction, while the Rust side still holds it. It is the rule every Ruby
//! extension lives by ("Ruby objects must be kept on the stack", as magnus put
//! it), left to the extension; this lint checks it, by type.
//!
//! It reads the type of every place a value is kept: a binding, a parameter,
//! a return type, a field, a static - and every expression, so a temporary
//! that is never bound (`f(vec![v])`, `Box::new(move || v)` before it is
//! made a `Box<dyn Fn>`) is reported too. An expression is reported once,
//! at the outermost one that holds the container, and not at all where its
//! value goes straight into a place that is itself checked (a `let`'s
//! initialiser, a function's result). A closure holds what it captures by
//! value.
//!
//! What is not reported: a value behind a reference or a raw pointer (the
//! value itself is elsewhere), and one inside `bridge::rb::Opaque`, which is
//! registered as a mark root. Nor what the types do not show: a value behind
//! `dyn Trait` once coerced (its expression before the coercion is checked),
//! a generic function's body for the `T` it is called with, or a raw `VALUE`
//! (an integer to the lint). That last is why no crate but `nupur_sys` can
//! hold one: it re-exports no `VALUE`, and keeps `Value::as_raw`,
//! `Value::from_raw` and its integer conversions of a value crate-private,
//! so Nupur holds a value only as `nupur_sys::value::Value`, which this lint
//! counts (`SYS_VALUES`). The values a wrapped object keeps alive are its
//! slots - `nupur_sys::value::Root`s, which `nupur::typed` owns and marks,
//! outside the payload, so a payload holds none at all.

use crate::{RUBY_VALUE_ON_HEAP, traits};
use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, ExprKind, FnDecl, Item, ItemKind, LetStmt, Node};
use rustc_lint::LateContext;
use rustc_middle::ty::{self, Ty, TypeVisitableExt};
use rustc_span::Span;
use std::collections::HashSet;

/// A type that keeps a value where the GC sees it by other means: the
/// crate's `Opaque`, a value registered as a mark root.
fn shields(cx: &LateContext<'_>, did: DefId) -> bool {
    crate::ours(cx, did) && cx.tcx.item_name(did).as_str() == "Opaque"
}

/// A std type whose contents live in a heap allocation.
fn heap_container<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>) -> bool {
    if ty.is_box() {
        return true;
    }
    let ty::Adt(adt, _) = ty.kind() else {
        return false;
    };
    matches!(
        cx.tcx
            .get_diagnostic_name(adt.did())
            .map(|s| s.as_str().to_owned())
            .as_deref(),
        Some(
            "Vec"
                | "VecDeque"
                | "HashMap"
                | "HashSet"
                | "BTreeMap"
                | "BTreeSet"
                | "Rc"
                | "Arc"
                | "LinkedList"
                | "BinaryHeap"
        )
    )
}

/// `nupur_sys`'s own value types, which hold a `VALUE` and implement no
/// trait of the layer's: a value, and a slot's `Root` (marked only in the
/// slot of the object that holds it, so no more to be kept elsewhere).
const SYS_VALUES: &[&str] = &["nupur_sys::value::Value", "nupur_sys::value::Root"];

fn is_ruby_value<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>, repr: Option<DefId>) -> bool {
    if let ty::Adt(adt, _) = ty.kind()
        && SYS_VALUES.contains(&crate::paths::path_of(cx, adt.did()).as_str())
    {
        return true;
    }
    /* The layer's trait, where the crate has it: `nupur_sys` has none, and
     * is held by `SYS_VALUES` alone. */
    repr.is_some_and(|repr| {
        !ty.has_escaping_bound_vars() && clippy_utils::ty::implements_trait(cx, ty, repr, &[])
    })
}

/// Whether `ty` holds a Ruby value by value (not behind a reference or a
/// shield), and so wherever `ty` itself is kept.
fn holds_value<'tcx>(
    cx: &LateContext<'tcx>,
    ty: Ty<'tcx>,
    repr: Option<DefId>,
    seen: &mut HashSet<Ty<'tcx>>,
) -> bool {
    if !seen.insert(ty) {
        return false;
    }
    if is_ruby_value(cx, ty, repr) {
        return true;
    }
    match ty.kind() {
        ty::Adt(adt, args) => {
            if shields(cx, adt.did()) {
                return false;
            }
            if heap_container(cx, ty) {
                /* Its contents are behind a pointer: the container does not
                 * hold them by value - `on_heap` reports what it holds. */
                return false;
            }
            adt.all_fields()
                .any(|f| holds_value(cx, f.ty(cx.tcx, args).skip_normalization(), repr, seen))
        }
        ty::Tuple(tys) => tys.iter().any(|t| holds_value(cx, t, repr, seen)),
        ty::Array(t, _) | ty::Slice(t) => holds_value(cx, *t, repr, seen),
        /* What a closure captures by value is its fields. */
        ty::Closure(_, args) => args
            .as_closure()
            .upvar_tys()
            .iter()
            .any(|t| holds_value(cx, t, repr, seen)),
        _ => false,
    }
}

/// The heap container somewhere in `ty` (by value, not behind a reference)
/// whose contents hold a Ruby value, if any.
fn on_heap<'tcx>(
    cx: &LateContext<'tcx>,
    ty: Ty<'tcx>,
    repr: Option<DefId>,
    seen: &mut HashSet<Ty<'tcx>>,
) -> Option<Ty<'tcx>> {
    if !seen.insert(ty) {
        return None;
    }
    match ty.kind() {
        ty::Adt(adt, args) => {
            if shields(cx, adt.did()) {
                return None;
            }
            if heap_container(cx, ty) {
                for arg in args.types() {
                    if holds_value(cx, arg, repr, &mut HashSet::new()) {
                        return Some(ty);
                    }
                    if let Some(inner) = on_heap(cx, arg, repr, seen) {
                        return Some(inner);
                    }
                }
                return None;
            }
            adt.all_fields()
                .find_map(|f| on_heap(cx, f.ty(cx.tcx, args).skip_normalization(), repr, seen))
        }
        ty::Tuple(tys) => tys.iter().find_map(|t| on_heap(cx, t, repr, seen)),
        ty::Array(t, _) | ty::Slice(t) => on_heap(cx, *t, repr, seen),
        ty::Closure(_, args) => args
            .as_closure()
            .upvar_tys()
            .iter()
            .find_map(|t| on_heap(cx, t, repr, seen)),
        _ => None,
    }
}

fn check_ty<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>, span: Span, what: &str) {
    let (repr, _) = traits(cx);
    if let Some(container) = on_heap(cx, ty, repr, &mut HashSet::new()) {
        report(cx, container, span, what);
    }
}

fn report<'tcx>(cx: &LateContext<'tcx>, container: Ty<'tcx>, span: Span, what: &str) {
    span_lint_and_help(
        cx,
        RUBY_VALUE_ON_HEAP,
        span,
        format!("{what} keeps a Ruby value in `{container}`, where the GC does not see it"),
        None,
        "keep Ruby values on the stack or in a Ruby Array; for a wrapped object, \
         in one of its slots (`TypedType`'s `roots`)",
    );
}

/// Whether the ADT `did` implements a wrapped-object data trait.
fn is_wrapped_data<'tcx>(cx: &LateContext<'tcx>, did: LocalDefId, wrapped: &[DefId]) -> bool {
    let ty = cx
        .tcx
        .type_of(did)
        .instantiate_identity()
        .skip_normalization();
    wrapped
        .iter()
        .any(|&t| clippy_utils::ty::implements_trait(cx, ty, t, &[]))
}

pub(crate) fn check_item<'tcx>(cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
    let (repr, wrapped) = traits(cx);
    match item.kind {
        ItemKind::Static(..) | ItemKind::Const(..) => {
            let is_const = matches!(item.kind, ItemKind::Const(..));
            let mut ty = cx
                .tcx
                .type_of(item.owner_id)
                .instantiate_identity()
                .skip_normalization();
            /* A `thread_local!` expands to a CONST `LocalKey<T>` whose `T`
             * lives in thread-local storage - no more seen by the GC than a
             * static. Its storage statics sit inside std's expansion, where
             * a lint is suppressed, so the key is where it is caught, and at
             * its name: the one span of it the user wrote. Any other const
             * is a value, copied where used, and is left alone. */
            let mut span = item.span;
            let mut local_key = false;
            if let ty::Adt(adt, args) = ty.kind() {
                if cx.tcx.item_name(adt.did()).as_str() == "LocalKey"
                    && matches!(cx.tcx.crate_name(adt.did().krate).as_str(), "std")
                {
                    if let Some(inner) = args.types().next() {
                        ty = inner;
                        local_key = true;
                        if let Some(ident) = item.kind.ident() {
                            span = ident.span;
                        }
                    }
                }
            }
            if is_const && !local_key {
                return;
            }
            if holds_value(cx, ty, repr, &mut HashSet::new()) {
                span_lint_and_help(
                    cx,
                    RUBY_VALUE_ON_HEAP,
                    span,
                    "a static holds a Ruby value, which the GC does not see".to_owned(),
                    None,
                    "keep it in `bridge::rb::Opaque` (a registered root)",
                );
            } else {
                check_ty(cx, ty, span, "this static");
            }
        }
        ItemKind::Struct(..) | ItemKind::Enum(..) | ItemKind::Union(..) => {
            let did = item.owner_id.def_id;
            let adt = cx.tcx.adt_def(did);
            let data = is_wrapped_data(cx, did, &wrapped);
            let args = ty::GenericArgs::identity_for_item(cx.tcx, did);
            for field in adt.all_fields() {
                let fty = field.ty(cx.tcx, args).skip_normalization();
                let span = cx.tcx.def_span(field.did);
                if data && holds_value(cx, fty, repr, &mut HashSet::new()) {
                    span_lint_and_help(
                        cx,
                        RUBY_VALUE_ON_HEAP,
                        span,
                        "a wrapped object's data holds a Ruby value, which no mark traces"
                            .to_owned(),
                        None,
                        "keep it in one of the object's slots (`TypedType`'s `roots`), or an `Opaque`",
                    );
                } else {
                    check_ty(cx, fty, span, "this field");
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn check_local<'tcx>(cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
    let ty = cx.typeck_results().pat_ty(local.pat);
    check_ty(cx, ty, local.span, "this binding");
}

pub(crate) fn check_fn<'tcx>(
    cx: &LateContext<'tcx>,
    _: FnKind<'tcx>,
    decl: &'tcx FnDecl<'tcx>,
    body: &'tcx Body<'tcx>,
    _: Span,
    did: LocalDefId,
) {
    if !matches!(cx.tcx.def_kind(did), DefKind::Fn | DefKind::AssocFn) {
        return;
    }
    let typeck = cx.tcx.typeck_body(body.id());
    for (param, hir) in body.params.iter().zip(decl.inputs) {
        check_ty(cx, typeck.pat_ty(param.pat), hir.span, "this parameter");
    }
    let sig = cx.tcx.fn_sig(did).instantiate_identity().skip_binder();
    if let rustc_hir::FnRetTy::Return(hir) = decl.output {
        check_ty(cx, sig.output(), hir.span, "this return type");
    }
}

/// A value-producing expression whose type keeps a Ruby value on the heap -
/// a temporary no binding names. A place (a binding, a field, a static, an
/// index or a dereference of one) is not: the storage it reads was checked
/// where it was declared.
pub(crate) fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let (repr, _) = traits(cx);
    if expr.is_syntactic_place_expr() {
        return;
    }
    let typeck = cx.typeck_results();
    let Some(container) = on_heap(cx, typeck.expr_ty(expr), repr, &mut HashSet::new()) else {
        return;
    };
    if reported_elsewhere(cx, expr, repr) {
        return;
    }
    /* `vec![v]` is std's expansion, where a lint is suppressed: reported
     * where the macro was called. */
    report(
        cx,
        container,
        expr.span.source_callsite(),
        "this expression",
    );
}

/// Whether `expr`'s value goes straight into what is reported in its place:
/// an enclosing expression that holds the container too (reported there,
/// once), a `let`'s binding, a function's result (its return type), or an
/// assignment to a place (checked where it is declared).
fn reported_elsewhere<'tcx>(
    cx: &LateContext<'tcx>,
    expr: &'tcx Expr<'tcx>,
    repr: Option<DefId>,
) -> bool {
    let mut id = expr.hir_id;
    loop {
        match cx.tcx.parent_hir_node(id) {
            /* A block's value is its tail; a statement's is dropped. */
            Node::Block(b) if b.expr.is_some_and(|e| e.hir_id == id) => id = b.hir_id,
            Node::Expr(p) => {
                return match p.kind {
                    /* A function's result is its return type; a closure's
                     * has none that is checked. */
                    ExprKind::Ret(_) => matches!(
                        cx.tcx.def_kind(cx.tcx.hir_enclosing_body_owner(p.hir_id)),
                        DefKind::Fn | DefKind::AssocFn
                    ),
                    ExprKind::Assign(lhs, rhs, _) => {
                        rhs.hir_id == id && lhs.is_syntactic_place_expr()
                    }
                    _ => {
                        !p.is_syntactic_place_expr()
                            && on_heap(
                                cx,
                                cx.typeck_results().expr_ty(p),
                                repr,
                                &mut HashSet::new(),
                            )
                            .is_some()
                    }
                };
            }
            Node::LetStmt(l) => return l.init.is_some_and(|e| e.hir_id == id),
            /* A function's body (its return type is checked), a static's or
             * a const's initialiser (the item is). */
            Node::Item(_) | Node::ImplItem(_) | Node::TraitItem(_) => return true,
            _ => return false,
        }
    }
}
