//! Evaluation entry points.
//!
//! Context state and registration live in [`super::ctx`]. This module owns the
//! public evaluation boundary and delegates the stateful implementation to the
//! context module.

use super::abi::{Error, Node, XPathValue};
use super::ctx::Context;
use core::ffi::c_int;

/// Evaluate a compiled XPath expression.
pub unsafe fn mkr_xpath_eval_compiled(
    ctx: *mut Context,
    ast: *mut Node,
    out_value: *mut XPathValue,
    out_error: *mut Error,
) -> c_int {
    super::ctx::eval_compiled(ctx, ast, out_value, out_error)
}

/// Evaluate a compiled expression using the first-match fast path when
/// possible, falling back to the full evaluator otherwise.
pub unsafe fn mkr_xpath_eval_compiled_first(
    ctx: *mut Context,
    ast: *mut Node,
    out_value: *mut XPathValue,
    out_error: *mut Error,
) -> c_int {
    super::ctx::eval_compiled_first(ctx, ast, out_value, out_error)
}
