//! The XML engine instance: the two entries the driver dispatches on, as the
//! `Xml` instantiation of the generic engine.
//!
//! The `_xml` suffix is from ext/makiri/xpath/mkr_xpath_engine_xml.c, where the
//! engine bodies were compiled once per representation and these were the only
//! two symbols that left the translation unit.

use super::abi::*;
use super::dom_xml::Xml;
use super::eval;
use core::ffi::{c_int, c_void};

/// Evaluate an AST against the context. 0 on success (filling `out`), -1 with
/// `*err` set otherwise.
/// # Safety
/// `ctx` and `ast` must be live, `ast` parsed for this context's host, and the
/// out-pointers writable; the caller holds the GVL.
pub unsafe fn mkr_eval_ast_xml(
    ctx: *mut Context,
    ast: *const Node,
    out: *mut Val,
    err: *mut Error,
) -> c_int {
    if eval::eval_ast::<Xml>(ctx, ast, out, err) {
        0
    } else {
        -1
    }
}

/// The `at_xpath` first-match short-circuit. Returns 1 when it handled the
/// expression (`*out_node` is the match or NULL), 0 when the shape is not
/// recognised and the caller should run the full evaluator, -1 on a budget
/// overrun with `*err` set.
/// # Safety
/// `ctx` and `ast` must be live, `ast` parsed for this context's host, and the
/// out-pointers writable; the caller holds the GVL.
pub unsafe fn mkr_try_first_match_xml(
    ctx: *mut Context,
    ast: *const Node,
    out_node: *mut *mut c_void,
    err: *mut Error,
) -> c_int {
    if out_node.is_null() {
        return 0;
    }
    match eval::try_first_match::<Xml>(ctx, ast, err) {
        Ok(None) => 0,
        Ok(Some(n)) => {
            *out_node = n.to_token() as *mut c_void;
            1
        }
        Err(()) => -1,
    }
}
