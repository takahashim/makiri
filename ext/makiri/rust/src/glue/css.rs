//! What a rejected CSS selector raises, for both representations.
//!
//! One wording, so `Makiri::HTML` and `Makiri::XML` answer a bad selector the
//! same way: `"<reason>: <selector>"`, where the reason is the engine's when it
//! has one ("unsupported CSS pseudo-class") and a plain "invalid CSS selector"
//! otherwise. Lexbor's matcher reports no reason, the lowering usually does.

#![forbid(unsafe_code)]

use magnus::{Error, Value};

use crate::init::EXC_CSS_SYNTAX_ERROR;

/// `Makiri::CSS::SyntaxError` naming `selector`.
pub fn syntax_error(selector: Value, reason: Option<&str>) -> Error {
    let reason = reason.unwrap_or("invalid CSS selector");
    Error::new(
        EXC_CSS_SYNTAX_ERROR.exception(),
        format!("{reason}: {selector}"),
    )
}
