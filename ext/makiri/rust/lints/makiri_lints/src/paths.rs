//! How the lints name what they look at: every path is the RESOLVED
//! definition, its crate's own name first, so a `use` alias, a re-export or
//! a macro expansion cannot hide a reference, and a comment or a string
//! cannot fake one.
//!
//! Four crates are linted, all of them Makiri's to ship: the extension
//! ([`EXTENSION`], `makiri::...`), its engine ([`ENGINE`],
//! `makiri_engine::...`), and the binding layer under it ([`NUPUR`],
//! [`NUPUR_SYS`]). The extension's modules and the engine's are named by
//! their path inside the crate (`bridge`, `glue`, `html::dom::...`) - the
//! extension re-exports the engine's at its root, so the two read alike;
//! the layer's by their full path (`nupur::raw`).
//!
//! The binding layer's own lints, and its helpers, are `nupur_lints`'; these
//! are Makiri's, which is why they may name its modules.

use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::{Expr, ExprKind, HirId};
use rustc_lint::LateContext;

/// The extension's crate name (the `[lib]` name of `makiri_rs`).
pub const EXTENSION: &str = "makiri";

/// The engine's crate name: Ruby-free by its dependencies.
pub const ENGINE: &str = "makiri_engine";

/// The binding layer's crate name.
pub const NUPUR: &str = "nupur";

/// The crate under the binding layer that alone depends on rb-sys.
pub const NUPUR_SYS: &str = "nupur_sys";

/// `did`'s path, its crate's name first: `makiri::bridge::html::wrap`,
/// `nupur::args::Kwargs`, `alloc::borrow::ToOwned::to_owned`. Unnamed parts
/// (an `impl` block, a closure) are left out.
pub fn path_of(cx: &LateContext<'_>, did: DefId) -> String {
    let mut parts = vec![cx.tcx.crate_name(did.krate).to_string()];
    for d in cx.tcx.def_path(did).data {
        if let Some(name) = d.data.get_opt_name() {
            parts.push(name.to_string());
        }
    }
    parts.join("::")
}

/// Whether the crate being linted is one of Makiri's four, not a build
/// script or a test target.
pub(crate) fn is_makiri(cx: &LateContext<'_>) -> bool {
    let krate = cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
    [EXTENSION, ENGINE, NUPUR, NUPUR_SYS].contains(&krate.as_str())
}

/// The path of the module the code at `hir_id` is in, crate first.
pub(crate) fn module_of(cx: &LateContext<'_>, hir_id: HirId) -> String {
    path_of(cx, cx.tcx.parent_module(hir_id).to_def_id())
}

/// `path` relative to the extension's crate or the engine's
/// (`makiri::bridge::x` is `bridge::x`, `makiri_engine::xml::y` is
/// `xml::y`); a path of any other crate - the layer's `nupur::...` included -
/// as it is.
fn in_extension_or_as_is(path: &str) -> &str {
    path.strip_prefix(ENGINE)
        .or_else(|| path.strip_prefix(EXTENSION))
        .and_then(|r| r.strip_prefix("::"))
        .unwrap_or(path)
}

/// Whether `path` is the module (or item) `m` or inside it: `m` an
/// extension module by its path inside the extension (`bridge`,
/// `lint_probe::bridge`), or a layer one by its full path (`nupur::raw`).
pub(crate) fn within(path: &str, m: &str) -> bool {
    in_extension_or_as_is(path)
        .strip_prefix(m)
        .is_some_and(|tail| tail.is_empty() || tail.starts_with("::"))
}

/// Whether `path` is the binding layer's: `nupur`'s or `nupur_sys`'s.
pub(crate) fn in_layer(path: &str) -> bool {
    within(path, NUPUR) || within(path, NUPUR_SYS)
}

/// Whether `path` is one of the four crates' own rather than a library's.
pub(crate) fn ours(path: &str) -> bool {
    in_layer(path)
        || [EXTENSION, ENGINE]
            .iter()
            .any(|k| path == *k || path.starts_with(&format!("{k}::")))
}

/// The function an expression calls, when it can be named: a path call or
/// a method call.
pub(crate) fn callee(cx: &LateContext<'_>, expr: &Expr<'_>) -> Option<DefId> {
    match expr.kind {
        ExprKind::Call(f, _) => match f.kind {
            ExprKind::Path(ref qpath) => match cx.qpath_res(qpath, f.hir_id) {
                Res::Def(DefKind::Fn | DefKind::AssocFn | DefKind::Ctor(..), did) => Some(did),
                _ => None,
            },
            _ => None,
        },
        ExprKind::MethodCall(..) => cx.typeck_results().type_dependent_def_id(expr.hir_id),
        _ => None,
    }
}
