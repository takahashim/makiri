//! The extension's layer boundaries, checked on the compiled code rather
//! than its text: every path here is the RESOLVED definition, so a `use`
//! alias, a re-export or a macro expansion cannot hide a reference, and a
//! comment or a string cannot fake one. (`script/check_unsafe_boundaries.rb`
//! checked these with regular expressions, and one of them - `RUBY_LAYER` -
//! held a backspace for months and matched nothing.)
//!
//! - `ruby_layer_in_engine`: the engine names nothing of the Ruby layer.
//! - `ruby_in_gvl_released_body`: a GVL-released body calls nothing that
//!   could reach Ruby.
//! - `infallible_allocation`: no allocation that aborts on OOM.
//! - `raising_call_outside_protect`: a C function that can raise is called
//!   only inside `nupur::raw::call`'s closure (which is private to
//!   `nupur::raw`, so that is also where every raising call is).
//! - `raw_ruby_outside_bridge`: Ruby's C API is named only in `bridge/`, and
//!   a raw `VALUE` - the type, or a C global of Ruby's, which holds one - only
//!   in the files that are the typed values.
//! - `runs_callers_ruby`: a call that would run the caller's Ruby code, each
//!   one allowed where it is made, with its reason.
//! - `raw_node_crossing`: a borrowed HTML node handle made a lifetime-free
//!   `RawNode` in `bridge/`, each one allowed where it is made.
//! - `ruby_surface_in_bridge`: a Ruby method defined, or an argument list
//!   split, in `bridge/` - the glue's work.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::{Expr, ExprKind, HirId, Path, QPath};
use rustc_lint::LateContext;
use rustc_span::Span;

use crate::paths::{
    NUPUR, NUPUR_SYS, callee, in_engine_crate, in_layer, is_extension, module_of, ours, path_of,
    within,
};
use crate::{
    INFALLIBLE_ALLOCATION, RAW_NODE_CROSSING, RAW_RUBY_OUTSIDE_BRIDGE, RUBY_IN_GVL_RELEASED_BODY,
    RUBY_LAYER_IN_ENGINE, RUBY_SURFACE_IN_BRIDGE, RUNS_CALLERS_RUBY,
};

/// Whether `module` is the engine's: Ruby-free, Lexbor-free code that an
/// edit's closure and a GVL-released body may run. That is the engine crate,
/// which names no Ruby because it depends on none (so `ruby_layer_in_engine`
/// can find nothing there), and nupur's one Ruby-free module, the checked
/// copy Miri runs without a Ruby, which sits beside the Ruby layer in its
/// crate and is held to it here. `lint_probe::engine` stands in for one in
/// the lint's self-test.
fn in_engine(module: &str) -> bool {
    if !ours(module) {
        return false;
    }
    in_engine_crate(module)
        || within(module, "nupur::utf8_copy")
        || within(module, "lint_probe::engine")
}

/// Whether `module` is in `bridge/`; `lint_probe::bridge` stands in for one
/// in the self-test.
fn in_bridge(module: &str) -> bool {
    within(module, "bridge") || in_layer(module) || within(module, "lint_probe::bridge")
}

/// The modules that may spell a raw `VALUE`: the C calls, TypedData and
/// the GC callbacks, and the `bridge::rb` files that are the typed values.
const VALUE_HOMES: &[&str] = &[
    "nupur::raw",
    "nupur::typed",
    "nupur::rb::value",
    "nupur::rb::kept",
    "nupur::rb::object",
    "nupur::rb::ruby",
    "nupur::rb::method",
    "nupur_sys",
];

/// What the engine may not name: the Ruby layer (`bridge`, `glue`, `init`)
/// and Ruby's C API.
fn ruby_layer(path: &str) -> bool {
    path.starts_with("rb_sys::")
        || (ours(path)
            && ["bridge", "glue", "init", NUPUR, NUPUR_SYS]
                .iter()
                .any(|m| within(path, m))
            && !in_engine(path))
}

/* ---- ruby_layer_in_engine ---- */

pub fn check_path(cx: &LateContext<'_>, path: &Path<'_>, hir_id: HirId) {
    if !is_extension(cx) {
        return;
    }
    let Res::Def(kind, did) = path.res else {
        return;
    };
    report_ruby_layer(cx, did, path.span.source_callsite(), hir_id);
    report_raw_ruby(cx, kind, did, path.span.source_callsite(), hir_id);
}

/* ---- raw_ruby_outside_bridge ---- */

fn report_raw_ruby(cx: &LateContext<'_>, kind: DefKind, did: DefId, span: Span, hir_id: HirId) {
    let target = path_of(cx, did);
    if !target.starts_with("rb_sys::") {
        return;
    }
    let module = module_of(cx, hir_id);
    if in_engine(&module) {
        return; /* `ruby_layer_in_engine`'s */
    }
    if !in_bridge(&module) {
        span_lint_and_help(
            cx,
            RAW_RUBY_OUTSIDE_BRIDGE,
            span,
            format!("`{target}`, Ruby's C API, outside bridge/"),
            None,
            "the bridge owns raw values, typed data and the C calls that raise, so a \
             raise becomes an Err before it can longjmp past a destructor; add a typed \
             wrapper there",
        );
    } else if (target.ends_with("::VALUE") || is_value_global(cx, kind, did))
        && !VALUE_HOMES.iter().any(|m| within(&module, m))
    {
        span_lint_and_help(
            cx,
            RAW_RUBY_OUTSIDE_BRIDGE,
            span,
            "a raw `VALUE` outside the files that are the typed values",
            None,
            "a VALUE is an integer to Rust, which no lint can follow into a Vec or a \
             field; hold a bridge::rb value (Value, RString, ...) instead",
        );
    }
}

/// A C global of Ruby's that holds a value (`rb_cObject`, `rb_eArgError`):
/// a raw `VALUE` read without the type being spelled. `VALUE` is an alias of
/// an unsigned integer, which the type shows; another global (the
/// `ruby_api_version` array) is not one.
fn is_value_global(cx: &LateContext<'_>, kind: DefKind, did: DefId) -> bool {
    matches!(kind, DefKind::Static { .. })
        && matches!(
            cx.tcx
                .type_of(did)
                .instantiate_identity()
                .skip_normalization()
                .kind(),
            rustc_middle::ty::Uint(_)
        )
}

fn report_ruby_layer(cx: &LateContext<'_>, did: DefId, span: Span, hir_id: HirId) {
    if !in_engine(&module_of(cx, hir_id)) {
        return;
    }
    let target = path_of(cx, did);
    if ruby_layer(&target) {
        span_lint_and_help(
            cx,
            RUBY_LAYER_IN_ENGINE,
            span,
            format!("the engine names `{target}`, of the Ruby layer"),
            None,
            "the engine is Ruby-free: an edit's closure and a GVL-released body run it, \
             and neither may reach Ruby; move the Ruby half to bridge/ or glue/",
        );
    }
}

/* ---- the expression checks ---- */

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
    if !is_extension(cx) {
        return;
    }
    check_raw_node_crossing(cx, expr);
    if let ExprKind::MethodCall(..) = expr.kind
        && let Some(did) = callee(cx, expr)
    {
        report_ruby_layer(cx, did, expr.span.source_callsite(), expr.hir_id);
    }
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

    if runs_callers_ruby(&path) {
        span_lint_and_help(
            cx,
            RUNS_CALLERS_RUBY,
            expr.span.source_callsite(),
            format!("`{path}` runs the caller's Ruby code"),
            None,
            "Makiri runs none of its caller's Ruby: no argument is converted, no method \
             of a caller's object called; a call that must is allowed where it is made, \
             with its reason, holding no guard or copy across it",
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

    /* The body is the last argument (`without_gvl(ruby, body)`): matched
     * as `[body]` alone, this saw nothing once the token joined it. */
    if path == "nupur::gvl::without_gvl"
        && let ExprKind::Call(_, [.., body]) = expr.kind
    {
        check_gvl_released(cx, body);
    }
}

/// The calls that run the caller's Ruby code: Ruby's C API that converts or
/// calls (`to_s`, `to_str`, a method, a block) - rb-sys's own, or
/// `nupur_sys::raising`'s wrapper of it, named the same - and the bridge's
/// wrappers over it.
fn runs_callers_ruby(path: &str) -> bool {
    if let Some(name) = path
        .strip_prefix("rb_sys::")
        .or_else(|| path.strip_prefix("nupur_sys::raising::"))
        .and_then(|p| p.rsplit("::").next())
    {
        return matches!(
            name,
            "rb_String"
                | "rb_obj_as_string"
                | "rb_respond_to"
                | "rb_block_call"
                | "rb_call_super"
                | "rb_str_to_str"
                | "rb_check_string_type"
                | "rb_convert_type"
                | "rb_check_convert_type"
        ) || [
            "rb_funcall",
            "rb_yield",
            "rb_eval_string",
            "rb_string_value",
        ]
        .iter()
        .any(|p| name.starts_with(p));
    }
    /* By the path each is defined at (`path_of`), not the one it is
     * re-exported under: the raw wrappers are in `raw`'s submodules. */
    matches!(
        path,
        "nupur::raw::define::call_method"
            | "nupur::raw::iterate::yield_value"
            | "nupur::rb::class::Module::call_method"
            | "nupur::rb::method::yield_value"
    )
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

/// A GVL-released body: everything it calls must be the engine's or a
/// library's - never the Ruby layer, Ruby's C API, or a function value whose
/// target no one can see.
fn check_gvl_released<'tcx>(cx: &LateContext<'tcx>, body: &'tcx Expr<'tcx>) {
    let ExprKind::Closure(closure) = body.kind else {
        span_lint_and_help(
            cx,
            RUBY_IN_GVL_RELEASED_BODY,
            body.span,
            "a GVL-released body that is not a closure written here",
            None,
            "pass a closure, so what it calls can be checked",
        );
        return;
    };
    let inner = cx.tcx.hir_body(closure.body);
    let _ = clippy_utils::visitors::for_each_expr(cx.tcx, inner.value, |e| {
        if matches!(e.kind, ExprKind::Call(..) | ExprKind::MethodCall(..)) {
            let bad = match callee(cx, e) {
                Some(did) => {
                    let path = path_of(cx, did);
                    path.starts_with("rb_sys::")
                        || (ours(&path)
                            && !in_engine(&path)
                            && !within(&path, "lint_probe::engine"))
                }
                /* A call through a local function value or a pointer. */
                None => {
                    matches!(e.kind, ExprKind::Call(f, _) if !matches!(f.kind, ExprKind::Path(QPath::TypeRelative(..))))
                }
            };
            if bad {
                span_lint_and_help(
                    cx,
                    RUBY_IN_GVL_RELEASED_BODY,
                    e.span.source_callsite(),
                    "a GVL-released body calls what is not the engine's",
                    None,
                    "with the GVL released no Ruby may run, and only the engine is known \
                     Ruby-free; hoist this call out of the closure",
                );
            }
        }
        core::ops::ControlFlow::<()>::Continue(())
    });
}
