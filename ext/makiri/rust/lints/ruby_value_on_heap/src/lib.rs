#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! `ruby_value_on_heap`: a Ruby value (a magnus `ReprValue` type) kept where
//! Ruby's GC cannot see it.
//!
//! Ruby's GC finds a value by scanning the machine stack. One in a `Vec`, a
//! `Box`, a `HashMap` or any other heap allocation, in a `static`, or in the
//! data of a wrapped object (a `TypedData` / `Hooks` type, which lives in
//! Ruby's or Rust's heap) is invisible to that scan: the object can be
//! collected, or moved by compaction, while the Rust side still holds it.
//! magnus documents the rule ("Ruby objects must be kept on the stack") and
//! leaves it to its users; this lint checks it, by type.
//!
//! What is not reported: a value behind a reference or a raw pointer (the
//! value itself is elsewhere), and one inside magnus's `Opaque`, `BoxValue` or
//! `Lazy`, which exist to keep a value somewhere else and are marked or
//! registered for it. Makiri's `Held` holds a raw `VALUE`, not a `ReprValue`
//! type, and is traced by its owner.

extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, FnDecl, Item, ItemKind, LetStmt};
use rustc_lint::{LateContext, LateLintPass};
use rustc_middle::ty::{self, Ty, TypeVisitableExt};
use rustc_span::Span;
use std::cell::OnceCell;
use std::collections::HashSet;

dylint_linting::declare_late_lint! {
    /// ### What it does
    /// Reports a Ruby value (a type implementing magnus's `ReprValue`) inside
    /// a heap container (`Box`, `Vec`, `VecDeque`, `HashMap`, `HashSet`,
    /// `BTreeMap`, `BTreeSet`, `Rc`, `Arc`, ...), in a `static`, or directly
    /// in the data of a wrapped Ruby object (a `TypedData` or Makiri `Hooks`
    /// type).
    ///
    /// ### Why is this bad?
    /// Ruby's GC finds values by scanning the machine stack. A value kept on
    /// the heap is not seen: it can be collected, or moved by compaction,
    /// while Rust still uses it.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let kept: Vec<magnus::RString> = strings.collect();
    /// ```
    /// Use instead: keep the values on the stack, in a Ruby Array, or - in a
    /// wrapped object's data - in a field its mark traces (`Held`, `Opaque`).
    pub RUBY_VALUE_ON_HEAP,
    Deny,
    "a Ruby value kept where Ruby's GC cannot see it"
}

/// The traits the lint asks about, looked up once per crate.
#[derive(Default)]
struct Traits {
    repr_value: Option<DefId>,
    wrapped: Vec<DefId>,
}

thread_local! {
    static TRAITS: OnceCell<Traits> = const { OnceCell::new() };
}

fn traits<'tcx>(cx: &LateContext<'tcx>) -> (Option<DefId>, Vec<DefId>) {
    TRAITS.with(|cell| {
        let t = cell.get_or_init(|| {
            let mut t = Traits::default();
            for did in cx.tcx.all_traits_including_private() {
                let krate = cx.tcx.crate_name(did.krate);
                let name = cx.tcx.item_name(did);
                match (krate.as_str(), name.as_str()) {
                    ("magnus", "ReprValue") => t.repr_value = Some(did),
                    ("magnus", "TypedData") => t.wrapped.push(did),
                    (_, "Hooks") if did.is_local() => t.wrapped.push(did),
                    _ => {}
                }
            }
            t
        });
        (t.repr_value, t.wrapped.clone())
    })
}

/// A magnus type that keeps a value where the GC sees it by other means.
fn shields(cx: &LateContext<'_>, did: DefId) -> bool {
    cx.tcx.crate_name(did.krate).as_str() == "magnus"
        && matches!(
            cx.tcx.item_name(did).as_str(),
            "Opaque" | "BoxValue" | "Lazy"
        )
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

fn is_ruby_value<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>, repr: DefId) -> bool {
    !ty.has_escaping_bound_vars() && clippy_utils::ty::implements_trait(cx, ty, repr, &[])
}

/// Whether `ty` holds a Ruby value by value (not behind a reference or a
/// shield), and so wherever `ty` itself is kept.
fn holds_value<'tcx>(
    cx: &LateContext<'tcx>,
    ty: Ty<'tcx>,
    repr: DefId,
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
        _ => false,
    }
}

/// The heap container somewhere in `ty` (by value, not behind a reference)
/// whose contents hold a Ruby value, if any.
fn on_heap<'tcx>(
    cx: &LateContext<'tcx>,
    ty: Ty<'tcx>,
    repr: DefId,
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
        _ => None,
    }
}

fn check_ty<'tcx>(cx: &LateContext<'tcx>, ty: Ty<'tcx>, span: Span, what: &str) {
    let (Some(repr), _) = traits(cx) else {
        return;
    };
    if let Some(container) = on_heap(cx, ty, repr, &mut HashSet::new()) {
        span_lint_and_help(
            cx,
            RUBY_VALUE_ON_HEAP,
            span,
            format!("{what} keeps a Ruby value in `{container}`, where the GC does not see it"),
            None,
            "keep Ruby values on the stack or in a Ruby Array; in a wrapped object's data, \
             use a field its mark traces (`Held`, `Opaque`)",
        );
    }
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

impl<'tcx> LateLintPass<'tcx> for RubyValueOnHeap {
    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        let (Some(repr), wrapped) = traits(cx) else {
            return;
        };
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
                        "keep it in magnus's `Opaque` (marked) or register it",
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
                            "a wrapped object's data holds a Ruby value its mark does not trace"
                                .to_owned(),
                            None,
                            "hold it in a `Held` (traced by the owner's `Hooks::trace`) or an `Opaque`",
                        );
                    } else {
                        check_ty(cx, fty, span, "this field");
                    }
                }
            }
            _ => {}
        }
    }

    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        let ty = cx.typeck_results().pat_ty(local.pat);
        check_ty(cx, ty, local.span, "this binding");
    }

    fn check_fn(
        &mut self,
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
}
