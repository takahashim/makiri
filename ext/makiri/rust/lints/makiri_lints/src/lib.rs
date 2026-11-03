#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! Makiri's own lints: where it draws its lines - its allocation policy and
//! its glue and bridge - over the crates it ships (the extension, the
//! engine, and the binding layer under them). The layer's guarantees, which
//! hold for any extension on Nupur, are `nupur_lints`', beside this crate;
//! `rake dylint` loads the two together.
//!
//! - `infallible_allocation` ([`layout`]): an allocation that aborts on OOM.
//! - `raw_node_crossing` ([`layout`]): a node handle made a lifetime-free
//!   `RawNode` in `bridge/`.
//! - `ruby_surface_in_bridge` ([`layout`]): a Ruby method defined, or an
//!   argument list split, in `bridge/`.
//!
//! Paths are named as [`paths`] spells them. The self-test is
//! `nupur_lints`' too: the extension's probe (`src/lint_probe.rs`) holds
//! the cases of both libraries.

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_session;
extern crate rustc_span;

mod layout;
mod paths;

use rustc_hir::Expr;
use rustc_lint::{LateContext, LateLintPass, LintStore};
use rustc_session::{Session, declare_lint, declare_lint_pass};

dylint_linting::dylint_library!();

#[allow(clippy::no_mangle_with_rust_abi)]
#[unsafe(no_mangle)]
pub fn register_lints(sess: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[
        INFALLIBLE_ALLOCATION,
        RAW_NODE_CROSSING,
        RUBY_SURFACE_IN_BRIDGE,
    ]);
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(MakiriLints)));
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
    pub RAW_NODE_CROSSING,
    Deny,
    "a node handle made a lifetime-free RawNode in bridge/"
}

declare_lint! {
    pub RUBY_SURFACE_IN_BRIDGE,
    Deny,
    "a Ruby method defined, or an argument list split, in bridge/"
}

declare_lint_pass!(MakiriLints => [
    INFALLIBLE_ALLOCATION,
    RAW_NODE_CROSSING,
    RUBY_SURFACE_IN_BRIDGE,
]);

impl<'tcx> LateLintPass<'tcx> for MakiriLints {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        layout::check_expr(cx, expr);
    }
}
