//! The HTML engine instance: the same two entries as the XML instance, suffixed
//! `_html`, over the `Html` binding of the same generic engine. This is the
//! instance every `Node#xpath` on an HTML document goes through.

use super::abi::*;
use super::dom_html::Html;
use super::eval;
use core::ffi::{c_int, c_void};

/// Evaluate an AST against the context. 0 on success (filling `out`), -1 with
/// `*err` set otherwise.
///
/// # Safety
/// `ctx` and `ast` must be live, `ast` parsed for this context's host, and the
/// out-pointers writable; the caller holds the GVL.
pub unsafe fn mkr_eval_ast_html(
    ctx: *mut Context,
    ast: *const Node,
    out: *mut Val,
    err: ErrSink,
) -> c_int {
    match eval::eval_ast::<Html>(ctx, ast, err) {
        Ok(mut v) => {
            *out = v.take();
            0
        }
        Err(_) => -1,
    }
}

/// The `at_xpath` first-match short-circuit. Returns 1 when it handled the
/// expression (`*out_node` is the match or NULL), 0 when the shape is not
/// recognised and the caller should run the full evaluator, -1 on a budget
/// overrun with `*err` set.
///
/// # Safety
/// `ctx` and `ast` must be live, `ast` parsed for this context's host, and the
/// out-pointers writable; the caller holds the GVL.
pub unsafe fn mkr_try_first_match_html(
    ctx: *mut Context,
    ast: *const Node,
    out_node: *mut *mut c_void,
    err: ErrSink,
) -> c_int {
    if out_node.is_null() {
        return 0;
    }
    match eval::try_first_match::<Html>(ctx, ast, err) {
        Ok(None) => 0,
        Ok(Some(n)) => {
            *out_node = n as *mut c_void;
            1
        }
        Err(_) => -1,
    }
}
