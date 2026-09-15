//! The HTML engine instance: the same two entries as the XML instance, suffixed
//! `_html`, over the `Html` binding of the same generic engine. This is the
//! instance every `Node#xpath` on an HTML document goes through.

use super::abi::*;
use super::eval;
use crate::dom_adapter::html::HtmlDoc;
use core::ffi::c_void;

/// Evaluate an AST against the context, with the context node as the focus.
///
/// # Safety
/// `cx`'s document must be this instance's, and `ast` parsed for it; the caller
/// holds the GVL.
#[allow(clippy::result_large_err)]
pub unsafe fn eval_ast_html(
    cx: &Context,
    ast: &Ast,
    handler: Option<Handler>,
) -> Result<Val, Error> {
    eval::eval_ast::<HtmlDoc<'_>>(cx, ast, handler)
}

/// The `at_xpath` first-match short-circuit: `Some(node)` when it handled the
/// expression (a null node for no match), `None` when the shape is not
/// recognised and the caller should run the full evaluator.
///
/// # Safety
/// As [`eval_ast_html`].
#[allow(clippy::result_large_err)]
pub unsafe fn try_first_match_html(cx: &Context, ast: &Ast) -> Result<Option<*mut c_void>, Error> {
    eval::try_first_match::<HtmlDoc<'_>>(cx, ast)
}
