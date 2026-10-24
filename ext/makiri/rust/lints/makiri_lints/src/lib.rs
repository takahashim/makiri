#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! Makiri's own lint over its Ruby boundary:
//!
//! - `ruby_value_on_heap` ([`value_on_heap`]): a Ruby value kept where Ruby's
//!   GC cannot see it.
//!
//! (A second lint, `converted_argument`, checked that no registered method
//! took a type magnus would convert with the caller's Ruby. Since
//! `bridge::rb` replaced magnus, its `TryConvert` exists only for `Value`,
//! `bool` and Makiri's receiver types, so such a method does not compile.)
//!
//! `rake dylint` runs both, then their self-test (`src/lint_probe.rs`
//! in the extension crate).

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod value_on_heap;

use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, FnDecl, Item, LetStmt};
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_session::{Session, declare_lint, declare_lint_pass};
use rustc_span::Span;
use std::cell::OnceCell;

dylint_linting::dylint_library!();

#[allow(clippy::no_mangle_with_rust_abi)]
#[unsafe(no_mangle)]
pub fn register_lints(sess: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[RUBY_VALUE_ON_HEAP]);
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(MakiriLints)));
}

declare_lint! {
    /// ### What it does
    /// Reports a Ruby value (a type implementing Makiri's `ReprValue`) inside
    /// a heap container (`Box`, `Vec`, `VecDeque`, `HashMap`, `HashSet`,
    /// `BTreeMap`, `BTreeSet`, `Rc`, `Arc`, ...), in a `static` or
    /// `thread_local!`, or directly in the data of a wrapped Ruby object (a
    /// Makiri `Hooks` type).
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
    /// wrapped object's data - in a field its mark traces (`Held`, `Opaque`).
    pub RUBY_VALUE_ON_HEAP,
    Deny,
    "a Ruby value kept where Ruby's GC cannot see it"
}

declare_lint_pass!(MakiriLints => [RUBY_VALUE_ON_HEAP]);

impl<'tcx> LateLintPass<'tcx> for MakiriLints {
    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx Item<'tcx>) {
        value_on_heap::check_item(cx, item);
    }

    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
        value_on_heap::check_local(cx, local);
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
/// `bridge::typed::Hooks`.
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
                    "Hooks" => t.wrapped.push(did),
                    _ => {}
                }
            }
            t
        });
        (t.repr_value, t.wrapped.clone())
    })
}
