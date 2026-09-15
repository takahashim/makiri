//! The XML engine instance: the two entries the driver dispatches on, as the
//! `Xml` instantiation of the generic engine.
//!
//! The `_xml` suffix is from ext/makiri/xpath/mkr_xpath_engine_xml.c, where the
//! engine bodies were compiled once per representation and these were the only
//! two symbols that left the translation unit.

use super::abi::*;
use super::dom_xml::Xml;
use super::eval;
use super::own::OwnedVal;
use core::ffi::c_void;

/// Evaluate an AST against the context, with the context node as the focus.
///
/// # Safety
/// `ctx` and `ast` must be live and `ast` parsed for this context's host; the
/// caller holds the GVL.
pub unsafe fn eval_ast_xml(
    ctx: *mut Context,
    ast: *const Node,
    err: ErrSink,
) -> Result<OwnedVal, Reported> {
    eval::eval_ast::<Xml>(ctx, ast, err)
}

/// The `at_xpath` first-match short-circuit: `Some(node)` when it handled the
/// expression (a null node for no match), `None` when the shape is not
/// recognised and the caller should run the full evaluator.
///
/// # Safety
/// As [`eval_ast_xml`].
pub unsafe fn try_first_match_xml(
    ctx: *mut Context,
    ast: *const Node,
    err: ErrSink,
) -> Result<Option<*mut c_void>, Reported> {
    Ok(eval::try_first_match::<Xml>(ctx, ast, err)?.map(|n| n.to_token() as *mut c_void))
}
