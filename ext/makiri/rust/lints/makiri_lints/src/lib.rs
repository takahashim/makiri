#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! Makiri's own lint over its Ruby boundary:
//!
//! - `ruby_value_on_heap` ([`value_on_heap`]): a Ruby value kept where Ruby's
//!   GC cannot see it.
//! - The layer boundaries ([`boundaries`]): `ruby_layer_in_engine`,
//!   `unguarded_no_unwind_site`, `ruby_in_gvl_released_body`,
//!   `infallible_allocation`, `raising_call_outside_protect`,
//!   `raw_ruby_outside_bridge`, `runs_callers_ruby`, `raw_node_crossing`,
//!   `ruby_surface_in_bridge`, `discarded_ruby_error`, `nupur_names_its_host`.
//!
//! (A second lint, `converted_argument`, checked that no registered method
//! took a type magnus would convert with the caller's Ruby. Since
//! `bridge::rb` replaced magnus, its `TryConvert` exists only for `Value`,
//! `bool` and Makiri's receiver types, so such a method does not compile.)
//!
//! `rake dylint` runs them all, then their self-test (`src/lint_probe.rs`
//! in the extension crate).

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod boundaries;
mod value_on_heap;

use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, FnDecl, HirId, Item, LetStmt, Path};
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_session::{Session, declare_lint, declare_lint_pass};
use rustc_span::Span;
use std::cell::OnceCell;

dylint_linting::dylint_library!();

#[allow(clippy::no_mangle_with_rust_abi)]
#[unsafe(no_mangle)]
pub fn register_lints(sess: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[
        RUBY_VALUE_ON_HEAP,
        RUBY_LAYER_IN_ENGINE,
        UNGUARDED_NO_UNWIND_SITE,
        RUBY_IN_GVL_RELEASED_BODY,
        INFALLIBLE_ALLOCATION,
        RAISING_CALL_OUTSIDE_PROTECT,
        RAW_RUBY_OUTSIDE_BRIDGE,
        RUNS_CALLERS_RUBY,
        RAW_NODE_CROSSING,
        RUBY_SURFACE_IN_BRIDGE,
        DISCARDED_RUBY_ERROR,
        NUPUR_NAMES_ITS_HOST,
    ]);
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(MakiriLints)));
}

declare_lint! {
    /// ### What it does
    /// Reports a Ruby value (a type implementing Makiri's `ReprValue`) inside
    /// a heap container (`Box`, `Vec`, `VecDeque`, `HashMap`, `HashSet`,
    /// `BTreeMap`, `BTreeSet`, `Rc`, `Arc`, ...), in a `static` or
    /// `thread_local!`, or directly in the data of a wrapped Ruby object (a
    /// Makiri `Payload` type).
    ///
    /// ### Why is this bad?
    /// Ruby's GC finds values by scanning the machine stack. A value kept on
    /// the heap is not seen: it can be collected, or moved by compaction,
    /// while Rust still uses it.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let kept: Vec<RString> = strings.collect();
    /// ```
    /// Use instead: keep the values on the stack, in a Ruby Array, or - in a
    /// wrapped object - in one of its slots (`TypedType`'s `roots`).
    pub RUBY_VALUE_ON_HEAP,
    Deny,
    "a Ruby value kept where Ruby's GC cannot see it"
}

declare_lint! {
    /// ### What it does
    /// Reports the engine (`html`, `xml`, `xpath`, ... - Ruby-free modules)
    /// naming anything of the Ruby layer: `bridge`, `glue`, `init`, `rb_sys`.
    ///
    /// ### Why is this bad?
    /// An edit's closure and a GVL-released body run engine code, and neither
    /// may reach Ruby: the one leaves the document's indexes dropped, the
    /// other has no GVL.
    pub RUBY_LAYER_IN_ENGINE,
    Deny,
    "the engine names something of the Ruby layer"
}

declare_lint! {
    /// ### What it does
    /// Reports an `extern "C"` function or a `Drop` whose body does not run
    /// under `no_unwind::must_not_unwind` (or a helper that does).
    ///
    /// ### Why is this bad?
    /// An unwind out of an `extern "C"` function aborts the process, as does
    /// a panic in a `Drop` the GC runs; `rake no_unwind` proves only the
    /// bodies that are sites.
    pub UNGUARDED_NO_UNWIND_SITE,
    Deny,
    "an extern \"C\" function or Drop not under must_not_unwind"
}

declare_lint! {
    /// ### What it does
    /// Reports a call, inside the closure given to `bridge::gvl::without_gvl`,
    /// to anything but the engine or a library.
    ///
    /// ### Why is this bad?
    /// With the GVL released no Ruby may run, and only the engine is known
    /// Ruby-free.
    pub RUBY_IN_GVL_RELEASED_BODY,
    Deny,
    "a GVL-released body calls what may reach Ruby"
}

declare_lint! {
    /// ### What it does
    /// Reports an allocation that aborts on OOM: `to_owned`, `to_vec`,
    /// `to_string`, `format!`, `from_utf8_lossy`, `into_owned`, `vec!`,
    /// `CString::new`.
    ///
    /// ### Why is this bad?
    /// An allocation failure must fail closed - an error, not the host
    /// process gone - so every allocation goes through `falloc`, and an
    /// exception message through `crate::msg::Msg`.
    pub INFALLIBLE_ALLOCATION,
    Deny,
    "an allocation that aborts on OOM"
}

declare_lint! {
    /// ### What it does
    /// Reports a call to a C function that can raise outside the closure
    /// given to `bridge::raw::call`.
    ///
    /// ### Why is this bad?
    /// A raise is a longjmp over every Rust frame up to the nearest
    /// `rb_protect`, which is undefined behaviour across a `Drop`.
    pub RAISING_CALL_OUTSIDE_PROTECT,
    Deny,
    "a raising C call outside bridge::raw::call"
}

declare_lint! {
    pub RAW_RUBY_OUTSIDE_BRIDGE,
    Deny,
    "Ruby's C API outside bridge/, or a raw VALUE outside the typed values"
}

declare_lint! {
    pub RUNS_CALLERS_RUBY,
    Deny,
    "a call that runs the caller's Ruby code"
}

declare_lint! {
    pub RAW_NODE_CROSSING,
    Deny,
    "a node handle made a lifetime-free RawNode in bridge/"
}

declare_lint! {
    pub RUBY_SURFACE_IN_BRIDGE,
    Deny,
    "a Ruby method defined, or an argument list split, in bridge/"
}

declare_lint! {
    pub DISCARDED_RUBY_ERROR,
    Deny,
    "a Result of a Ruby error whose Err is thrown away"
}

declare_lint! {
    pub NUPUR_NAMES_ITS_HOST,
    Deny,
    "the binding layer names something of the extension"
}

declare_lint_pass!(MakiriLints => [
    RUBY_VALUE_ON_HEAP,
    RUBY_LAYER_IN_ENGINE,
    UNGUARDED_NO_UNWIND_SITE,
    RUBY_IN_GVL_RELEASED_BODY,
    INFALLIBLE_ALLOCATION,
    RAISING_CALL_OUTSIDE_PROTECT,
    RAW_RUBY_OUTSIDE_BRIDGE,
    RUNS_CALLERS_RUBY,
    RAW_NODE_CROSSING,
    RUBY_SURFACE_IN_BRIDGE,
    DISCARDED_RUBY_ERROR,
    NUPUR_NAMES_ITS_HOST,
]);

impl<'tcx> LateLintPass<'tcx> for MakiriLints {
    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        value_on_heap::check_item(cx, item);
    }

    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        value_on_heap::check_local(cx, local);
        boundaries::check_local(cx, local);
    }

    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        kind: FnKind<'tcx>,
        decl: &'tcx FnDecl<'tcx>,
        body: &'tcx Body<'tcx>,
        span: Span,
        did: LocalDefId,
    ) {
        value_on_heap::check_fn(cx, kind, decl, body, span, did);
        boundaries::check_fn(cx, kind, body, span, did);
    }

    fn check_path(&mut self, cx: &LateContext<'tcx>, path: &Path<'tcx>, hir_id: HirId) {
        boundaries::check_path(cx, path, hir_id);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        boundaries::check_expr(cx, expr);
    }
}

/// The traits the lint asks about, looked up once per crate: the crate's
/// own, since `bridge::rb` is the crate's.
#[derive(Default)]
struct Traits {
    repr_value: Option<DefId>,
    wrapped: Vec<DefId>,
}

thread_local! {
    static TRAITS: OnceCell<Traits> = const { OnceCell::new() };
}

/// `bridge::rb::ReprValue`, and the wrapped-object data trait,
/// `bridge::typed::Payload`.
fn traits<'tcx>(cx: &LateContext<'tcx>) -> (Option<DefId>, Vec<DefId>) {
    TRAITS.with(|cell| {
        let t = cell.get_or_init(|| {
            let mut t = Traits::default();
            for did in cx.tcx.all_traits_including_private() {
                if !did.is_local() {
                    continue;
                }
                match cx.tcx.item_name(did).as_str() {
                    "ReprValue" => t.repr_value = Some(did),
                    "Payload" => t.wrapped.push(did),
                    _ => {}
                }
            }
            t
        });
        (t.repr_value, t.wrapped.clone())
    })
}
