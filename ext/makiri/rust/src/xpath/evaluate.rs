//! Evaluation entry points.
//!
//! Context state and registration live in [`super::ctx`]. This module owns the
//! public evaluation boundary and delegates the stateful implementation to the
//! context module.

use super::abi::{Error, Node, XPathValue};
use super::ctx::Context;
use core::ffi::c_int;

/// Evaluate a compiled XPath expression.
///
/// # Safety
/// `ctx`, `ast`, `out_value` and `out_error` must be live pointers from the
/// matching constructors; the caller holds the GVL for the whole call.
pub unsafe fn xpath_eval_compiled(
    ctx: *mut Context,
    ast: *mut Node,
    out_value: *mut XPathValue,
    out_error: *mut Error,
) -> c_int {
    super::ctx::eval_compiled(ctx, ast, out_value, out_error)
}

/// Evaluate a compiled expression using the first-match fast path when
/// possible, falling back to the full evaluator otherwise.
///
/// # Safety
/// As [`xpath_eval_compiled`].
pub unsafe fn xpath_eval_compiled_first(
    ctx: *mut Context,
    ast: *mut Node,
    out_value: *mut XPathValue,
    out_error: *mut Error,
) -> c_int {
    super::ctx::eval_compiled_first(ctx, ast, out_value, out_error)
}
