//! How the lints name what they look at: every path is the RESOLVED
//! definition, its crate's own name first, so a `use` alias, a re-export or
//! a macro expansion cannot hide a reference, and a comment or a string
//! cannot fake one.
//!
//! Four crates are linted: the binding layer, `nupur`, whose items are
//! spelled `nupur::...` whichever crate names them, and the crate under it
//! that alone depends on rb-sys ([`NUPUR_SYS`], `nupur_sys::...`); the
//! extension built on it
//! ([`extension`]), whose items are `makiri::...`; and the engine the
//! extension is the Ruby surface of ([`ENGINE`]), whose items are
//! `makiri_engine::...`. The layer's lints name only `nupur::` paths (and
//! rb-sys's), so a copy of the layer under another extension changes
//! [`extension`] alone (`NUPUR_LINTS_EXTENSION`); the extension's own lints name its modules, and the
//! engine's, by their path inside the crate (`bridge`, `glue`,
//! `html::dom::...`) - the extension re-exports the engine's at its root, so
//! the two read alike.

use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::{Expr, ExprKind, HirId};
use rustc_lint::LateContext;

/// The binding layer's crate name.
pub const NUPUR: &str = "nupur";

/// The crate under the binding layer that alone depends on rb-sys: the
/// `rb_protect` thunk, `$!`, the `no_unwind` guards, and Ruby's C through
/// it. Held to the layer's rules, as part of the layer.
pub const NUPUR_SYS: &str = "nupur_sys";

/// The extension's crate name: `makiri`, unless `NUPUR_LINTS_EXTENSION`
/// names another - an extension written on Nupur outside Makiri runs these
/// lints over itself by setting it (`NUPUR_LINTS_EXTENSION=nupur_json cargo
/// dylint --path .../nupur_lints`). The layer's lints then hold it to the
/// layer's rules; the extension's own lints name Makiri's modules (`bridge`,
/// `glue`), which another crate does not have, so they find nothing there.
/// A name that is not a crate name (`[a-z0-9_]+`) is ignored, so a stray
/// value cannot widen what counts as ours.
pub fn extension() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        std::env::var("NUPUR_LINTS_EXTENSION")
            .ok()
            .filter(|n| {
                !n.is_empty()
                    && n.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .unwrap_or_else(|| "makiri".to_owned())
    })
}

/* ---- the layer's functions the lints know by name ---- */
/*
 * Each table names functions by the path they are DEFINED at (`path_of`),
 * not one they are re-exported under. A function the layer adds that
 * belongs in one must be added here, or the lint does not see it; `rake
 * dylint` fails unless its self-test reports every entry (each probe line
 * names one, and the reports name the function), so an entry that stops
 * matching - renamed, moved - fails the task rather than going quiet.
 */

/// The layer's functions that run the caller's Ruby code - a method of the
/// caller's object, a block - reported at each call (`runs_callers_ruby`):
/// the raw wrappers of Ruby's C, and the API over them.
pub const CALLERS_RUBY_RUNNERS: &[&str] = &[
    "nupur::raw::define::call_method",
    "nupur::raw::define::call_public",
    "nupur::raw::define::respond_to",
    "nupur::raw::iterate::yield_value",
    "nupur::rb::call::public_method",
    "nupur::rb::call::responds_to",
    "nupur::rb::call::call_method",
    "nupur::rb::method::frame::yield_value",
];

/// The layer's functions that run their last argument, a closure, with the
/// GVL released (`without_gvl(ruby, body)`): it must touch no Ruby
/// (`ruby_in_gvl_released_body`).
pub const GVL_RELEASED_CALLS: &[&str] = &[
    "nupur::gvl::without_gvl",
    "nupur::gvl::without_gvl_cancellable",
];

/// The layer's methods that lend a String's bytes to their one argument, a
/// closure, held as a GVL-released body is: nothing in it may touch Ruby
/// while the slice is out.
pub const BYTES_LENDING_METHODS: &[&str] = &["nupur::string::read::with_bytes"];

/// The engine's crate name: Ruby-free by its dependencies, so the code a
/// GVL-released body and an edit's closure may call.
pub const ENGINE: &str = "makiri_engine";

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

/// Whether the crate being linted is the extension, its engine or its binding
/// layer, not a build script or a test target.
pub(crate) fn is_extension(cx: &LateContext<'_>) -> bool {
    let krate = cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
    [extension(), ENGINE, NUPUR, NUPUR_SYS].contains(&krate.as_str())
}

/// Whether Makiri's own policy lints (the extension's layout and its
/// allocation rule, not the layer's guarantees) apply to the crate being
/// linted: everywhere in Makiri, and nowhere in another extension that set
/// [`extension`] to itself - its allocation policy is its own, and it has no
/// `falloc` to allocate through.
pub(crate) fn makiri_policy_applies(cx: &LateContext<'_>) -> bool {
    let krate = cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
    extension() == "makiri" || krate.as_str() != extension()
}

/// The path of the module the code at `hir_id` is in, crate first.
pub(crate) fn module_of(cx: &LateContext<'_>, hir_id: HirId) -> String {
    path_of(cx, cx.tcx.parent_module(hir_id).to_def_id())
}

/// `path` relative to the extension's crate or the engine's
/// (`makiri::bridge::x` is `bridge::x`, `makiri_engine::xml::y` is
/// `xml::y`); a path of any other crate - the layer's `nupur::...` included -
/// as it is.
pub(crate) fn in_extension_or_as_is(path: &str) -> &str {
    path.strip_prefix(ENGINE)
        .or_else(|| path.strip_prefix(extension()))
        .and_then(|r| r.strip_prefix("::"))
        .unwrap_or(path)
}

/// Whether `path` is the engine crate's own.
pub(crate) fn in_engine_crate(path: &str) -> bool {
    path == ENGINE || path.starts_with(&format!("{ENGINE}::"))
}

/// Whether `path` is the module (or item) `m` or inside it: `m` an
/// extension module by its path inside the extension (`bridge`,
/// `lint_probe::engine`), or a layer one by its full path (`nupur::raw`).
pub(crate) fn within(path: &str, m: &str) -> bool {
    in_extension_or_as_is(path)
        .strip_prefix(m)
        .is_some_and(|tail| tail.is_empty() || tail.starts_with("::"))
}

/// Whether `path` is one of the three crates' own - the extension's, the
/// engine's or the layer's - rather than a library's.
pub(crate) fn ours(path: &str) -> bool {
    in_layer(path) || path.starts_with(&format!("{}::", extension())) || in_engine_crate(path)
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

/// Whether `path` is the binding layer's: `nupur`'s or `nupur_sys`'s.
pub(crate) fn in_layer(path: &str) -> bool {
    within(path, NUPUR) || within(path, NUPUR_SYS)
}
