//! The XML engine instance's exported symbols - what
//! ext/makiri/xpath/mkr_xpath_engine_xml.c compiles to.
//!
//! That translation unit is one merged copy of the three engine bodies with the
//! node-access macros bound to `mkr_xml_node_t`, and only two symbols leave it:
//! the entries the driver dispatches on, suffixed `_xml`. Here the same two are
//! the `Xml` instantiation of the generic engine.

use super::abi::*;
use super::dom_xml::Xml;
use super::eval;
use core::ffi::{c_int, c_void};

/// Evaluate an AST against the context. 0 on success (filling `out`), -1 with
/// `*err` set otherwise.
/// # Safety
/// A C entry point: the contract is the one at its declaration in
/// ext/makiri/xpath/mkr_xpath*.h.
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
/// A C entry point: the contract is the one at its declaration in
/// ext/makiri/xpath/mkr_xpath*.h.
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
