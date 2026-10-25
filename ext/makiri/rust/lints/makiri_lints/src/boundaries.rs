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
//!   only inside `bridge::raw::call`'s closure.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::DefId;
use rustc_hir::intravisit::FnKind;
use rustc_hir::{Body, Expr, ExprKind, HirId, Node, Path, QPath};
use rustc_lint::LateContext;
use rustc_span::Span;
use rustc_span::def_id::LocalDefId;

use crate::{
    INFALLIBLE_ALLOCATION, RAISING_CALL_OUTSIDE_PROTECT, RUBY_IN_GVL_RELEASED_BODY,
    RUBY_LAYER_IN_ENGINE, UNGUARDED_NO_UNWIND_SITE,
};

/* ---- paths ---- */

/// `did`'s path, its crate's name first: `makiri::html::tree::parse_html`,
/// `rb_sys::rb_str_new`, `alloc::borrow::ToOwned::to_owned`. Unnamed parts
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

/// Whether the crate being linted is the extension itself, not its build
/// script or a test target.
fn is_extension(cx: &LateContext<'_>) -> bool {
    cx.tcx.crate_name(rustc_span::def_id::LOCAL_CRATE).as_str() == "makiri"
}

/// The path of the module the code at `hir_id` is in, crate first.
fn module_of(cx: &LateContext<'_>, hir_id: HirId) -> String {
    path_of(cx, cx.tcx.parent_module(hir_id).to_def_id())
}

/// The modules of the engine: Ruby-free, Lexbor-free code that an edit's
/// closure and a GVL-released body may run. `lint_probe::engine` stands in
/// for one in the lint's self-test.
const ENGINE: &[&str] = &[
    "xml", "xpath", "css", "selectors", "html", "html_dom", "child_index", "dom_rules",
    "falloc", "cbuf", "cutf8", "limits", "node_type", "ptr_table", "text", "token",
    "utf8_input", "raw_vec", "utf8_copy", "msg", "css_match",
];

fn in_engine(module: &str) -> bool {
    let Some(rest) = module.strip_prefix("makiri::") else {
        return false;
    };
    rest.starts_with("lint_probe::engine")
        || ENGINE.iter().any(|m| rest == *m || rest.starts_with(&format!("{m}::")))
}

/// What the engine may not name: the Ruby layer (`bridge`, `glue`, `init`)
/// and Ruby's C API.
fn ruby_layer(path: &str) -> bool {
    path.starts_with("rb_sys::")
        || ["bridge", "glue", "init"]
            .iter()
            .any(|m| path == format!("makiri::{m}") || path.starts_with(&format!("makiri::{m}::")))
}

/// The function an expression calls, when it can be named: a path call or
/// a method call.
fn callee(cx: &LateContext<'_>, expr: &Expr<'_>) -> Option<DefId> {
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

/// The C functions that can raise, by name: `script/check_unsafe_boundaries.rb`'s
/// `RAISING_API`, which still counts their call sites.
const RAISING: &[&str] = &[
    "rb_raise", "rb_exc_raise", "rb_jump_tag", "rb_check_typeddata", "rb_sym2str",
    "rb_check_frozen", "rb_range_beg_len", "rb_thread_check_ints", "rb_intern3", "rb_intern",
    "rb_utf8_str_new", "rb_enc_interned_str", "rb_str_dup", "rb_str_subseq", "rb_enc_associate",
    "rb_str_encode", "rb_to_encoding", "rb_enc_find_index", "rb_data_typed_object_wrap",
    "rb_data_typed_object_zalloc", "ruby_xmalloc", "ruby_xrealloc2", "rb_ary_new_capa",
    "rb_ary_push", "rb_hash_new", "rb_hash_aset", "rb_ll2inum", "rb_ull2inum", "rb_float_new",
    "rb_id2sym", "rb_str_cat", "rb_enumeratorize_with_size", "rb_memerror", "rb_hash_foreach",
    "rb_yield", "rb_define_class_under", "rb_define_module_under", "rb_define_module",
    "rb_const_set", "rb_const_get", "rb_include_module", "rb_define_method",
    "rb_define_private_method", "rb_define_singleton_method", "rb_define_module_function",
    "rb_exc_new_str", "rb_exc_fatal", "rb_big2ll", "rb_big2ull", "rb_hash_lookup2",
    "rb_gc_register_mark_object", "rb_module_new", "rb_range_values", "rb_funcallv",
    "rb_ivar_set",
];

/// The functions of `bridge::raw` that raise on purpose, outside `call`:
/// the injected raise, and the three that end a method's trampoline.
const RAISES_ON_PURPOSE: &[&str] = &["injected_raise", "jump_tag", "exc_raise", "exc_fatal"];

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    if let ExprKind::MethodCall(..) = expr.kind
        && let Some(did) = callee(cx, expr)
    {
        report_ruby_layer(cx, did, expr.span.source_callsite(), expr.hir_id);
    }
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let path = path_of(cx, did);

    if INFALLIBLE.iter().any(|p| if p.starts_with("::") { path.ends_with(p) } else { path == *p })
        && !path.starts_with("makiri::")
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

    if let Some(name) = path.strip_prefix("rb_sys::")
        && let Some(name) = name.rsplit("::").next()
        && RAISING.contains(&name)
        && !inside_protect(cx, expr.hir_id)
    {
        span_lint_and_help(
            cx,
            RAISING_CALL_OUTSIDE_PROTECT,
            expr.span.source_callsite(),
            format!("`{name}` can raise, and is called outside `bridge::raw::call`"),
            None,
            "a raise is a longjmp over every Rust frame up to the nearest rb_protect; \
             make it a bridge::raw wrapper that runs it inside call(|| ...)",
        );
    }

    if path == "makiri::bridge::gvl::without_gvl"
        && let ExprKind::Call(_, [body]) = expr.kind
    {
        check_gvl_released(cx, body);
    }
}

/// Whether the call at `hir_id` is inside the closure handed to
/// `bridge::raw::call`, or in one of the functions that raise on purpose.
fn inside_protect(cx: &LateContext<'_>, hir_id: HirId) -> bool {
    let mut child = hir_id;
    for (id, node) in cx.tcx.hir_parent_iter(hir_id) {
        match node {
            Node::Expr(e) => {
                if let ExprKind::Call(f, args) = e.kind
                    && args.iter().any(|a| a.hir_id == child)
                    && let ExprKind::Path(ref qpath) = f.kind
                    && let Res::Def(_, did) = cx.qpath_res(qpath, f.hir_id)
                    && path_of(cx, did) == "makiri::bridge::raw::call"
                {
                    return true;
                }
            }
            Node::Item(item) => {
                let p = path_of(cx, item.owner_id.to_def_id());
                return RAISES_ON_PURPOSE
                    .iter()
                    .any(|f| p == format!("makiri::bridge::raw::{f}"));
            }
            _ => {}
        }
        child = id;
    }
    false
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
                None => matches!(e.kind, ExprKind::Call(f, _) if !matches!(f.kind, ExprKind::Path(QPath::TypeRelative(..)))),
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
    "makiri::bridge::rb::method::finish",
    "makiri::bridge::rb::method::finish_cary",
    "makiri::bridge::rb::method::run_init",
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
    let guarded = clippy_utils::visitors::for_each_expr(cx.tcx, body.value, |e| {
        match callee(cx, e) {
            Some(d) if GUARDS.contains(&path_of(cx, d).as_str()) => {
                core::ops::ControlFlow::Break(())
            }
            _ => core::ops::ControlFlow::Continue(()),
        }
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
    let Some(impl_did) = cx.tcx.inherent_impl_of_assoc(did.to_def_id()).or_else(|| cx.tcx.trait_impl_of_assoc(did.to_def_id())) else {
        return false;
    };
    cx.tcx
        .impl_opt_trait_ref(impl_did)
        .is_some_and(|t| Some(t.skip_binder().def_id) == cx.tcx.lang_items().drop_trait())
}
