//! How the lints name what they look at: every path is the RESOLVED
//! definition, its crate's own name first, so a `use` alias, a re-export or
//! a macro expansion cannot hide a reference, and a comment or a string
//! cannot fake one.
//!
//! The crates linted are the binding layer, `nupur`, whose items are
//! spelled `nupur::...` whichever crate names them, and the crate under it
//! that alone depends on rb-sys ([`NUPUR_SYS`], `nupur_sys::...`); the
//! extension built on it ([`extension`]); and any crate of the extension's
//! it declares Ruby-free ([`ruby_free`]). The lints name only the layer's
//! paths and rb-sys's: what they know of an extension is its
//! [`Config`], read from the `[nupur_lints]` table of the `dylint.toml` at
//! the root of the workspace being linted.

use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::{Expr, ExprKind, HirId};
use rustc_lint::LateContext;
use rustc_session::Session;
use serde::Deserialize;
use std::sync::OnceLock;

/// The binding layer's crate name.
pub const NUPUR: &str = "nupur";

/// The crate under the binding layer that alone depends on rb-sys: the
/// `rb_protect` thunk, `$!`, the `no_unwind` guards, and Ruby's C through
/// it. Held to the layer's rules, as part of the layer.
pub const NUPUR_SYS: &str = "nupur_sys";

/// What an extension tells the lints of itself, in its workspace's
/// `dylint.toml`; every key may be left out:
///
/// ```toml
/// [nupur_lints]
/// extension = "my_ext"
/// ruby_free = ["my_ext_core"]
/// no_unwind_guards = ["my_ext_core::no_unwind::must_not_unwind"]
/// ```
///
/// An unknown key, or a name that is not a crate name, fails the build
/// rather than being ignored: a stray value must not narrow what is checked.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The crate built on the layer. Left out, it is the crate being linted
    /// (unless that is the layer itself, or a build script).
    extension: Option<String>,
    /// The extension's crates that depend on no Ruby. They are linted too,
    /// for what any code must keep (an `extern "C"` function or a `Drop`
    /// under a no-unwind guard), and a body run with the GVL released may
    /// call them.
    #[serde(default)]
    ruby_free: Vec<String>,
    /// Functions of the extension's that put a body under a no-unwind proof
    /// of its own, beside the layer's (`unguarded_no_unwind_site`). Each is
    /// a path as [`path_of`] spells it, and the extension's build must
    /// prove the bodies it guards, as `rake no_unwind` does the layer's.
    #[serde(default)]
    no_unwind_guards: Vec<String>,
}

/// The configuration, with the extension resolved.
struct Resolved {
    extension: Option<String>,
    ruby_free: Vec<String>,
    no_unwind_guards: Vec<String>,
}

static CONFIG: OnceLock<Resolved> = OnceLock::new();

fn is_crate_name(n: &str) -> bool {
    !n.is_empty()
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Reads the configuration once per compilation, from `register_lints`
/// (after `dylint_linting::init_config`).
pub(crate) fn init(sess: &Session) {
    let config: Config = dylint_linting::config_or_default("nupur_lints");
    for n in config.extension.iter().chain(&config.ruby_free) {
        assert!(
            is_crate_name(n),
            "nupur_lints: `{n}` in dylint.toml is not a crate name"
        );
    }
    let extension = config.extension.or_else(|| {
        sess.opts.crate_name.clone().filter(|n| {
            is_crate_name(n)
                && ![NUPUR, NUPUR_SYS].contains(&n.as_str())
                && !n.starts_with("build_script_")
        })
    });
    let _ = CONFIG.set(Resolved {
        extension,
        ruby_free: config.ruby_free,
        no_unwind_guards: config.no_unwind_guards,
    });
}

fn config() -> &'static Resolved {
    CONFIG.get_or_init(|| Resolved {
        extension: None,
        ruby_free: Vec::new(),
        no_unwind_guards: Vec::new(),
    })
}

/// The extension's crate name, if there is one: [`Config`]'s `extension`,
/// or the crate being linted.
pub fn extension() -> Option<&'static str> {
    config().extension.as_deref()
}

/// The extension's Ruby-free crates ([`Config`]'s `ruby_free`).
pub fn ruby_free() -> &'static [String] {
    &config().ruby_free
}

/// The extension's own no-unwind guards ([`Config`]'s `no_unwind_guards`).
pub fn no_unwind_guards() -> &'static [String] {
    &config().no_unwind_guards
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

/// `did`'s path, its crate's name first: `my_ext::glue::parse`,
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

/// Whether the crate being linted is the layer's, the extension's or one
/// of its Ruby-free crates - not a build script or a test target.
pub(crate) fn is_extension(cx: &LateContext<'_>) -> bool {
    let krate = cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE);
    let krate = krate.as_str();
    [NUPUR, NUPUR_SYS].contains(&krate)
        || extension() == Some(krate)
        || ruby_free().iter().any(|r| r == krate)
}

/// The path of the module the code at `hir_id` is in, crate first.
pub(crate) fn module_of(cx: &LateContext<'_>, hir_id: HirId) -> String {
    path_of(cx, cx.tcx.parent_module(hir_id).to_def_id())
}

/// Whether `path` is the crate `krate`'s own.
fn in_crate(path: &str, krate: &str) -> bool {
    path.strip_prefix(krate)
        .is_some_and(|tail| tail.is_empty() || tail.starts_with("::"))
}

/// Whether `path` is the extension's crate's own.
pub(crate) fn in_extension(path: &str) -> bool {
    extension().is_some_and(|e| in_crate(path, e))
}

/// Whether `path` is one of the extension's Ruby-free crates' own.
pub(crate) fn in_ruby_free_crate(path: &str) -> bool {
    ruby_free().iter().any(|r| in_crate(path, r))
}

/// `path` relative to the extension's crate (`ext::lint_probe::x` is
/// `lint_probe::x`); a path of any other crate - the layer's `nupur::...`
/// included - as it is.
fn in_extension_or_as_is(path: &str) -> &str {
    extension()
        .and_then(|e| path.strip_prefix(e))
        .and_then(|r| r.strip_prefix("::"))
        .unwrap_or(path)
}

/// Whether `path` is the module (or item) `m` or inside it: `m` an
/// extension module by its path inside the extension (`lint_probe`), or a
/// layer one by its full path (`nupur::raw`).
pub(crate) fn within(path: &str, m: &str) -> bool {
    in_extension_or_as_is(path)
        .strip_prefix(m)
        .is_some_and(|tail| tail.is_empty() || tail.starts_with("::"))
}

/// Whether `path` is the layer's, the extension's or one of its Ruby-free
/// crates' own, rather than a library's.
pub(crate) fn ours(path: &str) -> bool {
    in_layer(path) || in_extension(path) || in_ruby_free_crate(path)
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
    in_crate(path, NUPUR) || in_crate(path, NUPUR_SYS)
}
