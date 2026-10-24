#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! Makiri's own lints over the magnus boundary, each a rule magnus states or
//! implies and leaves to its users:
//!
//! - `ruby_value_on_heap` ([`value_on_heap`]): a Ruby value kept where Ruby's
//!   GC cannot see it.
//! - `converted_argument` ([`converted_argument`]): a registered method whose
//!   argument magnus converts with the caller's Ruby (`to_str`, `to_int`, ...).
//!
//! `rake dylint` runs both, then their self-test (`src/lint_probe.rs`
//! in the extension crate).

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod converted_argument;
mod value_on_heap;

use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, FnDecl, Item, LetStmt};
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_session::{Session, declare_lint, declare_lint_pass};
use rustc_span::Span;
use std::cell::OnceCell;

dylint_linting::dylint_library!();

#[allow(clippy::no_mangle_with_rust_abi)]
#[unsafe(no_mangle)]
pub fn register_lints(sess: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[RUBY_VALUE_ON_HEAP, CONVERTED_ARGUMENT]);
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(MakiriLints)));
}

declare_lint! {
    /// ### What it does
    /// Reports a Ruby value (a type implementing magnus's `ReprValue`) inside
    /// a heap container (`Box`, `Vec`, `VecDeque`, `HashMap`, `HashSet`,
    /// `BTreeMap`, `BTreeSet`, `Rc`, `Arc`, ...), in a `static` or
    /// `thread_local!`, or directly in the data of a wrapped Ruby object (a
    /// `TypedData` or Makiri `Hooks` type).
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

declare_lint! {
    /// ### What it does
    /// Reports a method or function registered with magnus's `method!` /
    /// `function!` whose receiver or argument type is converted by a magnus
    /// `TryConvert` that can call Ruby: `RString` (`to_str`), `RArray`
    /// (`to_ary`), `RHash` (`to_hash`), `Integer` and the integers (`to_int`),
    /// `Float` and `f64` (`to_f`), `String`, `Symbol`, and so on.
    ///
    /// ### Why is this bad?
    /// The conversion runs the caller's Ruby inside the extension's frame, a
    /// method that can be redefined to re-enter the document or switch Fibers.
    /// Makiri converts nothing: an argument is a `Value`, read by
    /// `string_arg` / `name_arg` / `integer_arg`, which refuse anything but
    /// the class they name.
    ///
    /// ### Example
    /// ```rust,ignore
    /// fn set_name(ruby: &Ruby, rb_self: Value, name: RString) -> Result<Value, Error>
    /// ```
    /// Use instead: `name: Value`, then `string_arg(name, "name")?`.
    pub CONVERTED_ARGUMENT,
    Deny,
    "a registered method whose argument magnus converts with the caller's Ruby"
}

declare_lint_pass!(MakiriLints => [RUBY_VALUE_ON_HEAP, CONVERTED_ARGUMENT]);

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

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        converted_argument::check_expr(cx, expr);
    }
}

/// The traits the lints ask about, looked up once per crate.
#[derive(Default)]
struct Traits {
    repr_value: Option<DefId>,
    typed_data: Option<DefId>,
    wrapped: Vec<DefId>,
}

thread_local! {
    static TRAITS: OnceCell<Traits> = const { OnceCell::new() };
}

/// magnus's `ReprValue`, and the wrapped-object data traits: magnus's
/// `TypedData` and Makiri's own `Hooks`.
fn traits<'tcx>(cx: &LateContext<'tcx>) -> (Option<DefId>, Vec<DefId>) {
    with_traits(cx, |t| (t.repr_value, t.wrapped.clone()))
}

/// magnus's `TypedData`.
fn typed_data<'tcx>(cx: &LateContext<'tcx>) -> Option<DefId> {
    with_traits(cx, |t| t.typed_data)
}

fn with_traits<'tcx, R>(cx: &LateContext<'tcx>, f: impl FnOnce(&Traits) -> R) -> R {
    TRAITS.with(|cell| {
        f(cell.get_or_init(|| {
            let mut t = Traits::default();
            for did in cx.tcx.all_traits_including_private() {
                let krate = cx.tcx.crate_name(did.krate);
                let name = cx.tcx.item_name(did);
                match (krate.as_str(), name.as_str()) {
                    ("magnus", "ReprValue") => t.repr_value = Some(did),
                    ("magnus", "TypedData") => {
                        t.typed_data = Some(did);
                        t.wrapped.push(did);
                    }
                    (_, "Hooks") if did.is_local() => t.wrapped.push(did),
                    _ => {}
                }
            }
            t
        }))
    })
}
