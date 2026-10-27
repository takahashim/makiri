//! The extension's layer boundaries, checked on the compiled code rather
//! than its text: every path here is the RESOLVED definition, so a `use`
//! alias, a re-export or a macro expansion cannot hide a reference, and a
//! comment or a string cannot fake one. (`script/check_unsafe_boundaries.rb`
//! checked these with regular expressions, and one of them - `RUBY_LAYER` -
//! held a backspace for months and matched nothing.)
//!
//! - `ruby_layer_in_engine`: the engine names nothing of the Ruby layer.
//! - `unguarded_no_unwind_site`: every `extern "C"` function and every
//!   `Drop` runs its body under `no_unwind::must_not_unwind`.
//! - `ruby_in_gvl_released_body`: a GVL-released body calls nothing that
//!   could reach Ruby.
//! - `infallible_allocation`: no allocation that aborts on OOM.
//! - `raising_call_outside_protect`: a C function that can raise is called
//!   only inside `bridge::raw::call`'s closure (which is private to
//!   `bridge::raw`, so that is also where every raising call is).
//! - `raw_ruby_outside_bridge`: Ruby's C API is named only in `bridge/`, and
//!   a raw `VALUE` only in the files that are the typed values.
//! - `runs_callers_ruby`: a call that would run the caller's Ruby code, each
//!   one allowed where it is made, with its reason.
//! - `raw_node_crossing`: a borrowed HTML node handle made a lifetime-free
//!   `RawNode` in `bridge/`, each one allowed where it is made.
//! - `discarded_ruby_error`: a `Result<_, bridge::rb::Error>` whose `Err` is
//!   thrown away - `.ok()`, `.unwrap_or*`, `.is_ok()`, `.or(..)`, a
//!   `.map_err` / `.or_else` closure that ignores it, `let _ =` or
//!   `let _name =` (an `Error` itself too), a pattern
//!   that leaves it unbound (`if let Ok(..)`, `let Ok(..) else`, a `match`
//!   arm `Err(_)` or `_`), or an `Error` handed to `drop` / `mem::forget`: a
//!   raise or a `throw` in it would be lost, and the call taken as having
//!   answered. An `Err` becomes "no answer" only through `Error::rescue`,
//!   which names what it handles and hands every exit back; the extension
//!   `forbid`s the lint, so no `allow` reopens it there.
//! - `ruby_surface_in_bridge`: a Ruby method defined, or an argument list
//!   split, in `bridge/` - the glue's work.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, ExprKind, HirId, LetStmt, PatKind, Path, QPath};
use rustc_lint::LateContext;
use rustc_span::Span;
use rustc_span::def_id::LocalDefId;

use crate::{
    DISCARDED_RUBY_ERROR, INFALLIBLE_ALLOCATION, RAW_NODE_CROSSING, RAW_RUBY_OUTSIDE_BRIDGE,
    RUBY_IN_GVL_RELEASED_BODY, RUBY_LAYER_IN_ENGINE, RUBY_SURFACE_IN_BRIDGE, RUNS_CALLERS_RUBY,
    UNGUARDED_NO_UNWIND_SITE,
};

/* ---- paths ---- */

/// `did`'s path, its crate's name first: `makiri::html::tree::parse_html`,
/// `rb_sys::rb_str_new`, `alloc::borrow::ToOwned::to_owned`. Unnamed parts
/// (an `impl` block, a closure) are left out.
///
/// The binding layer is a crate of its own, `nupur`, which the extension
/// names as `crate::nupur`; its items are spelled `makiri::nupur::...` here,
/// whichever crate is being linted, so one set of paths serves both.
pub fn path_of(cx: &LateContext<'_>, did: DefId) -> String {
    let krate = cx.tcx.crate_name(did.krate);
    let mut parts = vec![match krate.as_str() {
        NUPUR => "makiri::nupur".to_string(),
        k => k.to_string(),
    }];
    for d in cx.tcx.def_path(did).data {
        if let Some(name) = d.data.get_opt_name() {
            parts.push(name.to_string());
        }
    }
    parts.join("::")
}

/// The binding layer's crate name.
pub const NUPUR: &str = "nupur";

/// Whether the crate being linted is the extension or its binding layer, not
/// a build script or a test target.
pub(crate) fn is_extension(cx: &LateContext<'_>) -> bool {
    matches!(
        cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE).as_str(),
        "makiri" | NUPUR
    )
}

/// The path of the module the code at `hir_id` is in, crate first.
pub(crate) fn module_of(cx: &LateContext<'_>, hir_id: HirId) -> String {
    path_of(cx, cx.tcx.parent_module(hir_id).to_def_id())
}

/// The modules of the engine: Ruby-free, Lexbor-free code that an edit's
/// closure and a GVL-released body may run. `lint_probe::engine` stands in
/// for one in the lint's self-test.
const ENGINE: &[&str] = &[
    "xml",
    "xpath",
    "css",
    "selectors",
    "html",
    "html_dom",
    "child_index",
    "dom_rules",
    "falloc",
    "cbuf",
    "cutf8",
    "limits",
    "node_type",
    "ptr_table",
    "text",
    "token",
    "utf8_input",
    "utf8_copy",
    "css_match",
];

fn in_engine(module: &str) -> bool {
    let Some(rest) = module.strip_prefix("makiri::") else {
        return false;
    };
    rest.starts_with("lint_probe::engine")
        || ENGINE
            .iter()
            .any(|m| rest == *m || rest.starts_with(&format!("{m}::")))
}

/// Whether `module` is the crate's module `m` or inside it.
fn within(module: &str, m: &str) -> bool {
    module
        .strip_prefix("makiri::")
        .and_then(|rest| rest.strip_prefix(m))
        .is_some_and(|tail| tail.is_empty() || tail.starts_with("::"))
}

/// Whether `module` is in `bridge/`; `lint_probe::bridge` stands in for one
/// in the self-test.
fn in_bridge(module: &str) -> bool {
    within(module, "bridge") || within(module, "nupur") || within(module, "lint_probe::bridge")
}

/// The modules that may spell a raw `VALUE`: the C calls, TypedData and
/// the GC callbacks, and the `bridge::rb` files that are the typed values.
const VALUE_HOMES: &[&str] = &[
    "nupur::int",
    "nupur::raw",
    "nupur::typed",
    "nupur::rb::value",
    "nupur::rb::ruby",
    "nupur::rb::method",
];

/// What the engine may not name: the Ruby layer (`bridge`, `glue`, `init`)
/// and Ruby's C API.
fn ruby_layer(path: &str) -> bool {
    path.starts_with("rb_sys::")
        || ["bridge", "glue", "init", "nupur"]
            .iter()
            .any(|m| path == format!("makiri::{m}") || path.starts_with(&format!("makiri::{m}::")))
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

/* ---- ruby_layer_in_engine ---- */

pub fn check_path(cx: &LateContext<'_>, path: &Path<'_>, hir_id: HirId) {
    if !is_extension(cx) {
        return;
    }
    let Res::Def(_, did) = path.res else {
        return;
    };
    report_ruby_layer(cx, did, path.span.source_callsite(), hir_id);
    report_raw_ruby(cx, did, path.span.source_callsite(), hir_id);
}

/* ---- raw_ruby_outside_bridge ---- */

fn report_raw_ruby(cx: &LateContext<'_>, did: DefId, span: Span, hir_id: HirId) {
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
    } else if target.ends_with("::VALUE") && !VALUE_HOMES.iter().any(|m| within(&module, m)) {
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
    check_discarded_error(cx, expr);
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
    }) && !path.starts_with("makiri::")
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

    let defines = (path.starts_with("makiri::bridge::") || path.starts_with("makiri::nupur::"))
        && path.rsplit("::").next().is_some_and(is_definition);
    if defines || path.starts_with("makiri::glue::args::") {
        let module = module_of(cx, expr.hir_id);
        if in_bridge(&module)
            && !within(&module, "nupur::rb::class")
            && !within(&module, "nupur::raw")
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

    if path == "makiri::nupur::gvl::without_gvl"
        && let ExprKind::Call(_, [body]) = expr.kind
    {
        check_gvl_released(cx, body);
    }
}

/// The calls that run the caller's Ruby code: Ruby's C API that converts or
/// calls (`to_s`, `to_str`, a method, a block), and the bridge's wrappers
/// over it.
fn runs_callers_ruby(path: &str) -> bool {
    if let Some(name) = path
        .strip_prefix("rb_sys::")
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
    matches!(
        path,
        "makiri::nupur::raw::call_method"
            | "makiri::nupur::raw::yield_value"
            | "makiri::nupur::rb::class::Module::call_method"
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

const RAW_NODE: &str = "makiri::html::dom::handle::RawNode";

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
        "makiri::html::dom::parsed::raw_node" => true,
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
                        || (path.starts_with("makiri::")
                            && !in_engine(&path)
                            && !path.starts_with("makiri::lint_probe::engine"))
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

/* ---- unguarded_no_unwind_site ---- */

/// The calls that put a body under `must_not_unwind`: it, and the
/// trampolines' and `Init_makiri`'s helpers, which do.
const GUARDS: &[&str] = &[
    "makiri::no_unwind::must_not_unwind",
    "makiri::nupur::no_unwind::must_not_unwind",
    "makiri::nupur::rb::method::finish",
    "makiri::nupur::rb::method::finish_cary",
    "makiri::nupur::rb::method::run_init",
];

pub fn check_fn<'tcx>(
    cx: &LateContext<'tcx>,
    kind: FnKind<'tcx>,
    body: &'tcx Body<'tcx>,
    span: Span,
    did: LocalDefId,
) {
    if !is_extension(cx) {
        return;
    }
    let site = match kind {
        FnKind::ItemFn(_, _, header) if !header.abi.is_rustic_abi() => "an `extern \"C\"` function",
        FnKind::Method(_, sig) if !sig.header.abi.is_rustic_abi() => "an `extern \"C\"` function",
        FnKind::Method(..) if is_drop(cx, did) => "a `Drop`",
        _ => return,
    };
    let guarded =
        clippy_utils::visitors::for_each_expr(cx.tcx, body.value, |e| match callee(cx, e) {
            Some(d) if GUARDS.contains(&path_of(cx, d).as_str()) => {
                core::ops::ControlFlow::Break(())
            }
            _ => core::ops::ControlFlow::Continue(()),
        })
        .is_some();
    if !guarded {
        span_lint_and_help(
            cx,
            UNGUARDED_NO_UNWIND_SITE,
            span,
            format!("{site} whose body does not run under `must_not_unwind`"),
            None,
            "an unwind out of it aborts the process; add a site to src/no_unwind.rs \
             and run the body through must_not_unwind, so `rake no_unwind` proves it",
        );
    }
}

fn is_drop(cx: &LateContext<'_>, did: LocalDefId) -> bool {
    let Some(impl_did) = cx
        .tcx
        .inherent_impl_of_assoc(did.to_def_id())
        .or_else(|| cx.tcx.trait_impl_of_assoc(did.to_def_id()))
    else {
        return false;
    };
    cx.tcx
        .impl_opt_trait_ref(impl_did)
        .is_some_and(|t| Some(t.skip_binder().def_id) == cx.tcx.lang_items().drop_trait())
}

/* ---- discarded_ruby_error ---- */

/// The methods that answer a `Result` without its error.
const DISCARDING: &[&str] = &[
    "ok",
    "unwrap_or",
    "unwrap_or_default",
    "unwrap_or_else",
    "map_or",
    "map_or_else",
    "is_ok",
    "is_err",
    "is_ok_and",
    "is_err_and",
    "or",
];

/// The methods whose closure is handed the `Err`, which it may ignore.
const HANDING_ERR: &[&str] = &["map_err", "or_else"];

/// Whether `ty` is `Result<_, makiri::nupur::rb::error::Error>`.
fn is_ruby_result<'tcx>(cx: &LateContext<'tcx>, ty: rustc_middle::ty::Ty<'tcx>) -> bool {
    let rustc_middle::ty::Adt(adt, args) = ty.kind() else {
        return false;
    };
    if path_of(cx, adt.did()) != "core::result::Result" {
        return false;
    }
    args.types().nth(1).is_some_and(|e| is_ruby_error(cx, e))
}

/// Whether `ty` is `makiri::nupur::rb::error::Error` itself.
fn is_ruby_error<'tcx>(cx: &LateContext<'tcx>, ty: rustc_middle::ty::Ty<'tcx>) -> bool {
    ty.ty_adt_def()
        .is_some_and(|e| path_of(cx, e.did()) == "makiri::nupur::rb::error::Error")
}

/// Whether `pat` binds nothing a reader would use: `_`, or a name that
/// starts with one.
fn ignores(pat: &rustc_hir::Pat<'_>) -> bool {
    match pat.kind {
        PatKind::Wild => true,
        PatKind::Binding(_, _, ident, None) => ident.name.as_str().starts_with('_'),
        _ => false,
    }
}

/// Whether `pat`, on a `Result`, leaves its `Err` unbound: `Ok(..)` (the
/// `Err` falls through), `Err(_)`, or `_`.
fn leaves_err(pat: &rustc_hir::Pat<'_>) -> bool {
    match pat.kind {
        PatKind::Wild => true,
        PatKind::TupleStruct(QPath::Resolved(_, path), fields, _) => {
            match path.segments.last().map(|s| s.ident.name.as_str()) {
                Some("Ok") => true,
                Some("Err") => fields.iter().all(ignores),
                _ => false,
            }
        }
        _ => false,
    }
}

fn report_discarded(cx: &LateContext<'_>, span: Span, how: &str) {
    span_lint_and_help(
        cx,
        DISCARDED_RUBY_ERROR,
        span,
        format!("a Ruby error is thrown away ({how})"),
        None,
        "an Err of bridge::rb::Error is a raise, a throw or a refusal: pass it on with `?`; \
         where dropping it is the answer, allow this with the reason",
    );
}

fn check_discarded_error<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if let ExprKind::MethodCall(seg, receiver, ..) = expr.kind
        && DISCARDING.contains(&seg.ident.name.as_str())
        && is_ruby_result(cx, cx.typeck_results().expr_ty(receiver))
    {
        report_discarded(
            cx,
            expr.span.source_callsite(),
            &format!(".{}()", seg.ident.name),
        );
    }
    /* `.map_err(|_| ..)` / `.or_else(|_| ..)`: the closure is handed the
     * `Err` and ignores it. */
    if let ExprKind::MethodCall(seg, receiver, [arg], _) = expr.kind
        && HANDING_ERR.contains(&seg.ident.name.as_str())
        && let ExprKind::Closure(closure) = arg.kind
        && cx
            .tcx
            .hir_body(closure.body)
            .params
            .first()
            .is_some_and(|p| ignores(p.pat))
        && is_ruby_result(cx, cx.typeck_results().expr_ty(receiver))
    {
        report_discarded(
            cx,
            expr.span.source_callsite(),
            &format!(".{}(|_| ..)", seg.ident.name),
        );
    }
    /* `if let Ok(..) = r` / `while let Ok(..) = r` / `if let Err(_) = r`:
     * the `Err` is left unbound. */
    if let ExprKind::Let(let_expr) = expr.kind
        && leaves_err(let_expr.pat)
        && is_ruby_result(cx, cx.typeck_results().expr_ty(let_expr.init))
    {
        report_discarded(cx, expr.span.source_callsite(), "if let");
    }
    /* A `match` arm `Err(_)` or `_`, on a Ruby result. `Ok(..)` arms are
     * fine here: a `match` must cover the `Err` too. */
    if let ExprKind::Match(scrutinee, arms, _) = expr.kind
        && is_ruby_result(cx, cx.typeck_results().expr_ty(scrutinee))
    {
        for arm in arms {
            let unbound = match arm.pat.kind {
                PatKind::Wild => true,
                PatKind::TupleStruct(..) => {
                    !matches!(arm.pat.kind, PatKind::TupleStruct(QPath::Resolved(_, path), ..)
                        if path.segments.last().is_some_and(|s| s.ident.name.as_str() == "Ok"))
                        && leaves_err(arm.pat)
                }
                _ => false,
            };
            if unbound {
                report_discarded(cx, arm.pat.span.source_callsite(), "a match arm Err(_)");
            }
        }
    }
    /* `drop(e)` / `mem::forget(e)` of an `Error` or a Ruby result. */
    if let ExprKind::Call(_, [arg]) = expr.kind
        && let Some(did) = callee(cx, expr)
        && matches!(
            path_of(cx, did).as_str(),
            "core::mem::drop" | "core::mem::forget"
        )
        && {
            let ty = cx.typeck_results().expr_ty(arg);
            is_ruby_error(cx, ty) || is_ruby_result(cx, ty)
        }
    {
        report_discarded(cx, expr.span.source_callsite(), "drop / mem::forget");
    }
}

pub fn check_local<'tcx>(cx: &LateContext<'tcx>, local: &'tcx LetStmt<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    /* `let _ = r` and `let _r = r`: a name that starts with `_` says it is
     * not read, and an `Error` bound so is as lost as a `Result`'s. */
    if ignores(local.pat)
        && let Some(init) = local.init
        && {
            let ty = cx.typeck_results().expr_ty(init);
            is_ruby_result(cx, ty) || is_ruby_error(cx, ty)
        }
    {
        report_discarded(cx, local.span.source_callsite(), "let _ =");
    }
    if local.els.is_some()
        && leaves_err(local.pat)
        && let Some(init) = local.init
        && is_ruby_result(cx, cx.typeck_results().expr_ty(init))
    {
        report_discarded(cx, local.span.source_callsite(), "let Ok(..) else");
    }
}
