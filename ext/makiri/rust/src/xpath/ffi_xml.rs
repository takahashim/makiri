//! The XML engine instance: the two entries the driver dispatches on, as the
//! `Xml` instantiation of the generic engine.
//!
//! The `_xml` suffix is from ext/makiri/xpath/mkr_xpath_engine_xml.c, where the
//! engine bodies were compiled once per representation and these were the only
//! two symbols that left the translation unit.

use super::abi::*;
use super::eval;
use super::own::OwnedVal;
use crate::xml::model as xml;
use core::ffi::c_void;

/// Evaluate an AST against the context, with the context node as the focus.
///
/// # Safety
/// `cx`'s document must be this instance's, and `ast` parsed for it; the caller
/// holds the GVL.
#[allow(clippy::result_large_err)]
pub unsafe fn eval_ast_xml(
    cx: &Context,
    ast: &Ast,
    handler: Option<Handler>,
) -> Result<OwnedVal, Error> {
    eval::eval_ast::<&xml::Document>(cx, ast, handler)
}

/// The `at_xpath` first-match short-circuit: `Some(node)` when it handled the
/// expression (a null node for no match), `None` when the shape is not
/// recognised and the caller should run the full evaluator.
///
/// # Safety
/// As [`eval_ast_xml`].
#[allow(clippy::result_large_err)]
pub unsafe fn try_first_match_xml(cx: &Context, ast: &Ast) -> Result<Option<*mut c_void>, Error> {
    eval::try_first_match::<&xml::Document>(cx, ast)
}
