//! Which of Ruby's C functions can raise, decided in one place, and the two
//! rules that follow from it.
//!
//! A raise is a `longjmp` over every Rust frame up to the nearest
//! `rb_protect`, undefined behaviour across a `Drop`, so every C call that
//! can raise runs inside the closure handed to `nupur::raw`'s `call`. The
//! tables here are of the functions that CANNOT raise - `NON_RAISING`, which
//! the C API raise survey finds no path to a longjmp for, and `REVIEWED`,
//! which it does and a person has read, each with the reason; every other
//! function of rb-sys is taken to raise - so a function nobody
//! has classified is refused until someone does, where a list of the ones
//! that raise let a missing entry (`rb_undef_alloc_func`, `rb_obj_freeze`)
//! through unseen.
//!
//! - `raising_call_outside_protect`: a function not in the table, called
//!   outside `call`'s closure. Exempt is `raw::protect`, the protection
//!   itself, `$!` and the calls that raise on purpose. A function that
//!   raises only when handed what a caller rules out (`rb_obj_is_kind_of`
//!   with a non-class) stays out of the table: its call site is an `allow`
//!   that says why.
//! - `unraising_call_in_raw`: what `nupur::raw` holds, read from the code. A
//!   call of Ruby's C in `raw` outside `call` (a read that raises nothing
//!   belongs with the values, `rb::value` / `rb::object`), and a `call` whose closure calls
//!   nothing that can raise (a protection that protects nothing): each is
//!   the table and the code disagreeing about a function.

use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::def::Res;
use rustc_hir::{Expr, ExprKind, HirId, Node};
use rustc_lint::LateContext;
use std::ops::ControlFlow;

use crate::paths::{callee, is_extension, module_of, path_of};
use crate::{RAISING_CALL_OUTSIDE_PROTECT, UNRAISING_CALL_IN_RAW};

/// Ruby's C functions that raise nothing, whatever a caller of the layer
/// hands them - each with NO path to a longjmp in the C API raise survey
/// (takahashim.github.io/ruby-c-apis: every public function, over the latest
/// patch release of each Ruby since 3.3 and HEAD, on Linux and macOS, from the
/// call graph of the compiled C). `script/check_non_raising.rb` holds the
/// table to the survey's vendored copy (`raise_survey.json`) and to Ruby's
/// headers: an entry the survey finds a path for, or whose doc names an
/// exception, or that never returns, fails `rake dylint`.
const NON_RAISING: &[&str] = &[
    /* Frees, and the memory counter. They may start no collection: Ruby's
     * objspace_malloc_increase_body collects for MEMOP_TYPE_MALLOC alone, and
     * these pass MEMOP_TYPE_FREE and MEMOP_TYPE_REALLOC. */
    "ruby_xfree",
    "rb_gc_adjust_memory_usage",
    /* Reads of the thread and its current frame. */
    "rb_errinfo",
    "rb_block_given_p",
    "rb_keyword_given_p",
    "ruby_native_thread_p",
    "ruby_stack_check",
    /* The running thread's extension config, written: methods defined from
     * here on may be called from any Ractor. */
    "rb_ext_ractor_safe",
    /* A check: false for a value of any other type. */
    "rb_typeddata_is_kind_of",
    /* Reads of an object: an Array element (nil past the end). */
    "rb_ary_entry",
    /* An object's class field, written: hiding. (Showing it again,
     * `rb_obj_reveal`, is not here: its write barrier can allocate.) */
    "rb_obj_hide",
    /* Encodings: the builtin three and their indices, an encoding's index,
     * and a lookup by index (NULL for none). */
    "rb_utf8_encoding",
    "rb_usascii_encoding",
    "rb_ascii8bit_encoding",
    "rb_utf8_encindex",
    "rb_usascii_encindex",
    "rb_ascii8bit_encindex",
    "rb_enc_from_index",
    "rb_enc_to_index",
];

/// Functions the survey finds a path for, which a person has read and finds
/// raise nothing, whoever calls them: each with the reason. The survey
/// resolves a call through a function pointer to ANY address-taken function
/// (`any`), so a path through one is no proof; its `typed` reading (a callee
/// whose type matches the call's) drops those, and what is left is a path the
/// source shows is not taken. A function that CAN raise, but not where the
/// extension calls it (`rb_attr_get` outside the main Ractor, `rb_gc_mark` when
/// memory runs out), is not here: the crate-wide table would let a new call
/// site inherit what was true of the old, so each such call carries its own
/// `allow` and reason. `check_non_raising.rb` fails an entry the survey no
/// longer finds a path for (move it up) and one without a reason.
const REVIEWED: &[(&str, &str)] = &[(
    "rb_thread_call_without_gvl2",
    "the survey's path is rb_nogvl -> vm_check_ints_blocking -> \
     rb_threadptr_execute_interrupts -> rb_threadptr_to_kill, which rb_nogvl takes \
     only when (flags & RB_NOGVL_INTR_FAIL) == 0 (thread.c); the `2` variant passes \
     that flag, so a pending interrupt stays pending, for any caller. The survey does \
     not follow a flag's value",
)];

/// Where the protection, `$!` and the raises on purpose live.
const PROTECT: &str = "nupur::raw::protect";

/// The modules held to `unraising_call_in_raw`: `raw` itself, and the
/// self-test's stand-in.
const RAW: &[&str] = &["nupur::raw", "nupur::lint_probe::raw"];

/// The protected call, and the self-test's stand-in.
const CALL: &[&str] = &["nupur::raw::protect::call", "nupur::lint_probe::raw::call"];

/// A function of rb-sys, by name, when `path` names one.
fn rb_sys_function(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("rb_sys::")?;
    if rest.starts_with("stable_api") {
        /* The stable API's inline reads, which raise nothing. */
        return Some("");
    }
    rest.rsplit("::").next()
}

fn may_raise(name: &str) -> bool {
    !name.is_empty() && !NON_RAISING.contains(&name) && !REVIEWED.iter().any(|(n, _)| *n == name)
}

pub fn check_expr<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    if !is_extension(cx) {
        return;
    }
    check_protected_closure(cx, expr);
    let Some(did) = callee(cx, expr) else {
        return;
    };
    let path = path_of(cx, did);
    let Some(name) = rb_sys_function(&path) else {
        return;
    };
    let module = module_of(cx, expr.hir_id);
    let in_protect = module == PROTECT || module.starts_with(&format!("{PROTECT}::"));
    let inside = inside_call(cx, expr.hir_id);
    if may_raise(name) && !inside && !in_protect {
        span_lint_and_help(
            cx,
            RAISING_CALL_OUTSIDE_PROTECT,
            expr.span.source_callsite(),
            format!("`{name}` can raise, and is called outside `raw::call`"),
            None,
            "a raise is a longjmp over every Rust frame up to the nearest rb_protect; make it \
             a nupur::raw wrapper that runs it inside call(|| ...) - or, for a function that \
             cannot raise, add it to makiri_lints' NON_RAISING (the survey finds no path) or REVIEWED (with the reason)",
        );
    }
    if RAW.contains(&module.as_str()) && !inside {
        let what = if name.is_empty() {
            "the stable API"
        } else {
            name
        };
        span_lint_and_help(
            cx,
            UNRAISING_CALL_IN_RAW,
            expr.span.source_callsite(),
            format!("`{what}` is called in `raw` outside `call`"),
            None,
            "`raw` holds the C calls made inside call(|| ...); a read that raises nothing \
             belongs with the values (rb::value, or rb::object for an object's ivars and class), and the protection itself in raw::protect",
        );
    }
}

/// A `call` whose closure calls nothing of Ruby's C that can raise.
fn check_protected_closure<'tcx>(cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
    let ExprKind::Call(f, [arg]) = expr.kind else {
        return;
    };
    if !is_call(cx, f) {
        return;
    }
    let ExprKind::Closure(closure) = arg.kind else {
        return;
    };
    let body = cx.tcx.hir_body(closure.body);
    let raises = clippy_utils::visitors::for_each_expr(cx.tcx, body.value, |e| {
        match callee(cx, e).map(|did| path_of(cx, did)) {
            Some(path) if rb_sys_function(&path).is_some_and(may_raise) => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    })
    .is_some();
    if !raises {
        span_lint_and_help(
            cx,
            UNRAISING_CALL_IN_RAW,
            expr.span.source_callsite(),
            "a protected call of nothing that can raise",
            None,
            "a C call that cannot raise needs no rb_protect; if this one can, it is missing \
             from makiri_lints' tables the other way (NON_RAISING or REVIEWED lists it)",
        );
    }
}

/// Whether `f`, a call's callee, is `raw`'s `call`.
fn is_call(cx: &LateContext<'_>, f: &Expr<'_>) -> bool {
    let ExprKind::Path(ref qpath) = f.kind else {
        return false;
    };
    let Res::Def(_, did) = cx.qpath_res(qpath, f.hir_id) else {
        return false;
    };
    CALL.contains(&path_of(cx, did).as_str())
}

/// Whether the expression at `hir_id` is inside the closure handed to `call`.
fn inside_call(cx: &LateContext<'_>, hir_id: HirId) -> bool {
    let mut child = hir_id;
    for (id, node) in cx.tcx.hir_parent_iter(hir_id) {
        match node {
            Node::Expr(e) => {
                if let ExprKind::Call(f, args) = e.kind
                    && args.iter().any(|a| a.hir_id == child)
                    && is_call(cx, f)
                {
                    return true;
                }
            }
            Node::Item(_) => return false,
            _ => {}
        }
        child = id;
    }
    false
}
