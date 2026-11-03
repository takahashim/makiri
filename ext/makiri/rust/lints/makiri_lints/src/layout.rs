//! Makiri's policy, checked on the compiled code rather than its text: every
//! path here is the RESOLVED definition, so a `use` alias, a re-export or a
//! macro expansion cannot hide a reference, and a comment or a string cannot
//! fake one.
//!
//! - `infallible_allocation`: no allocation that aborts on OOM.
//! - `raw_node_crossing`: a borrowed HTML node handle made a lifetime-free
//!   `RawNode` in `bridge/`, each one allowed where it is made.
//! - `ruby_surface_in_bridge`: a Ruby method defined, or an argument list
//!   split, in `bridge/` - the glue's work.
//!
//! Each holds the binding layer to it too, as code Makiri ships: the layer
//! allocates nothing that aborts, and counts as `bridge/` for where the Ruby
//! surface is made.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{Expr, ExprKind};
use rustc_lint::LateContext;

use crate::paths::{NUPUR, callee, in_layer, is_makiri, module_of, ours, path_of, within};
use crate::{INFALLIBLE_ALLOCATION, RAW_NODE_CROSSING, RUBY_SURFACE_IN_BRIDGE};

/// Whether `module` is in `bridge/`, or the binding layer under it;
/// `lint_probe::bridge` stands in for one in the self-test.
fn in_bridge(module: &str) -> bool {
    within(module, "bridge") || in_layer(module) || within(module, "lint_probe::bridge")
}

/// What allocates and aborts on OOM, by the path it resolves to.
const INFALLIBLE: &[&str] = &[
    "::ToOwned::to_owned",
    "::to_vec",
    "::ToString::to_string",
    "::fmt::format",
    "::String::from_utf8_lossy",
    "::Cow::into_owned",
    "alloc::vec::from_elem",
    "::into_vec",
    /* `vec![...]` on this toolchain: an uninit box, made a Vec. */
    "alloc::boxed::box_assume_init_into_vec_unsafe",
    "alloc::boxed::new_uninit",
    "::CString::new",
];

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_makiri(cx) {
        return;
    }
    check_raw_node_crossing(cx, expr);
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let path = path_of(cx, did);

    if INFALLIBLE.iter().any(|p| {
        if p.starts_with("::") {
            path.ends_with(p)
        } else {
            path == *p
        }
    }) && !ours(&path)
    {
        span_lint_and_help(
            cx,
            INFALLIBLE_ALLOCATION,
            /* At the call site: `format!` and `vec!` are std's macros, and
             * rustc keeps a lint quiet inside an external macro. */
            expr.span.source_callsite(),
            format!("`{path}` allocates and aborts the process on OOM"),
            None,
            "allocate through falloc (or, for an exception message, format_args! into \
             crate::msg::Msg); an allocation that can fail must fail closed",
        );
    }

    let defines = ours(&path)
        && (within(&path, "bridge") || within(&path, NUPUR))
        && path.rsplit("::").next().is_some_and(is_definition);
    /* Splitting an argument list: `nupur::args`' scan, its `Scanned` and
     * `Kwargs` - not its per-value type checks (`string_arg` and kin), which
     * the bridge's conversions use. */
    let splits_args = path
        .strip_prefix("nupur::args::")
        .is_some_and(|item| !item.ends_with("_arg"));
    if defines || splits_args {
        let module = module_of(cx, expr.hir_id);
        if in_bridge(&module)
            && !within(&module, "nupur::rb::class")
            && !within(&module, "nupur::raw")
            && !within(&module, "nupur::args")
        {
            span_lint_and_help(
                cx,
                RUBY_SURFACE_IN_BRIDGE,
                expr.span.source_callsite(),
                format!("`{path}` in bridge/: the Ruby surface is the glue's"),
                None,
                "define the method, or split its arguments, in glue/; a method defined \
                 in bridge/ splits a feature's Ruby half by where it needed unsafe",
            );
        }
    }
}

/// A function that defines a Ruby method: `define_method`,
/// `define_singleton_method`, `define_module_function`, ...
fn is_definition(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("define_") else {
        return false;
    };
    let rest = ["singleton_", "private_", "module_"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .unwrap_or(rest);
    rest == "method" || rest == "function"
}

/* ---- raw_node_crossing ---- */

const RAW_NODE: &str = "makiri_engine::html::dom::handle::RawNode";

/// `RawNode::from(node)` (as a call or a path, `.map(RawNode::from)`), a
/// `.into()` that makes one, and `HtmlNodeKey::raw_node`, in `bridge/`.
fn check_raw_node_crossing<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let did = match expr.kind {
        ExprKind::Path(ref qpath) => match cx.qpath_res(qpath, expr.hir_id) {
            Res::Def(DefKind::AssocFn, did) => did,
            _ => return,
        },
        ExprKind::MethodCall(..) => match cx.typeck_results().type_dependent_def_id(expr.hir_id) {
            Some(did) => did,
            None => return,
        },
        _ => return,
    };
    let path = path_of(cx, did);
    let made = |i: usize| {
        cx.typeck_results()
            .node_args(expr.hir_id)
            .types()
            .nth(i)
            .and_then(|t| t.ty_adt_def())
            .is_some_and(|a| path_of(cx, a.did()) == RAW_NODE)
    };
    let crossing = match path.as_str() {
        "core::convert::From::from" => made(0),
        "core::convert::Into::into" => made(1),
        "makiri_engine::html::dom::parsed::raw_node" => true,
        _ => false,
    };
    if crossing && in_bridge(&module_of(cx, expr.hir_id)) {
        span_lint_and_help(
            cx,
            RAW_NODE_CROSSING,
            expr.span.source_callsite(),
            "a node handle made a lifetime-free `RawNode` in bridge/",
            None,
            "a RawNode outlives the read it came from; reach for with_html_node, \
             HtmlParsed::mint_key or HtmlParsed::resolve, or allow this crossing here \
             with its reason",
        );
    }
}
