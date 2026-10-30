//! How the lints name what they look at: every path is the RESOLVED
//! definition, its crate's own name first, so a `use` alias, a re-export or
//! a macro expansion cannot hide a reference, and a comment or a string
//! cannot fake one.
//!
//! Two crates are linted: the binding layer, `nupur`, whose items are spelled
//! `nupur::...` whichever crate names them, and the extension built on it
//! ([`EXTENSION`]), whose items are `makiri::...`. The layer's lints name
//! only `nupur::` paths (and rb-sys's), so a copy of the layer under another
//! extension changes [`EXTENSION`] alone; the extension's own lints name its
//! modules by their path inside it (`bridge`, `glue`, `html::dom::...`).

use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::{Expr, ExprKind, HirId};
use rustc_lint::LateContext;

/// The binding layer's crate name.
pub const NUPUR: &str = "nupur";

/// The extension's crate name - the one name a copy of the layer's lints
/// under another extension changes.
pub const EXTENSION: &str = "makiri";

/// `did`'s path, its crate's name first: `makiri::html::tree::parse_html`,
/// `nupur::raw::define::call_method`, `rb_sys::rb_str_new`,
/// `alloc::borrow::ToOwned::to_owned`. Unnamed parts (an `impl` block, a
/// closure) are left out.
pub fn path_of(cx: &LateContext<'_>, did: DefId) -> String {
    let mut parts = vec![cx.tcx.crate_name(did.krate).to_string()];
    for d in cx.tcx.def_path(did).data {
        if let Some(name) = d.data.get_opt_name() {
            parts.push(name.to_string());
        }
    }
    parts.join("::")
}

/// Whether the crate being linted is the extension or its binding layer, not
/// a build script or a test target.
pub(crate) fn is_extension(cx: &LateContext<'_>) -> bool {
    let krate = cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
    krate.as_str() == EXTENSION || krate.as_str() == NUPUR
}

/// The path of the module the code at `hir_id` is in, crate first.
pub(crate) fn module_of(cx: &LateContext<'_>, hir_id: HirId) -> String {
    path_of(cx, cx.tcx.parent_module(hir_id).to_def_id())
}

/// `path` relative to the extension's crate (`makiri::bridge::x` is
/// `bridge::x`); a path of any other crate - the layer's `nupur::...`
/// included - as it is.
pub(crate) fn in_extension_or_as_is(path: &str) -> &str {
    path.strip_prefix(EXTENSION)
        .and_then(|r| r.strip_prefix("::"))
        .unwrap_or(path)
}

/// Whether `path` is the module (or item) `m` or inside it: `m` an
/// extension module by its path inside the extension (`bridge`,
/// `lint_probe::engine`), or a layer one by its full path (`nupur::raw`).
pub(crate) fn within(path: &str, m: &str) -> bool {
    in_extension_or_as_is(path)
        .strip_prefix(m)
        .is_some_and(|tail| tail.is_empty() || tail.starts_with("::"))
}

/// Whether `path` is one of the two crates' own - the extension's or the
/// layer's - rather than a library's.
pub(crate) fn ours(path: &str) -> bool {
    within(path, NUPUR) || path.starts_with(&format!("{EXTENSION}::"))
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
