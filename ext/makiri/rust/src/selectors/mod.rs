//! The CSS parser, ported from Lexbor's `css/syntax` (the tokenizer and the
//! rule machinery), `css/selectors` (the selector states) and the stylesheet
//! callbacks of `css/state.c` and `css/at_rule/state.c`: selectors into an
//! owned AST ([`parse`]), stylesheets into a rule arena over it
//! ([`parse_stylesheet`]).
//!
//! Ruby- and Lexbor-free, like the HTML parser port in `crate::html`: Lexbor's
//! own parser is kept only as the reference the tests compare against.

#![forbid(unsafe_code)]

pub mod ast;
pub mod parser;
pub mod pseudo;
pub mod rules;
mod specificity;
mod strtod;
pub mod token;
pub mod tokenizer;

pub use parser::{parse, parse_stylesheet, Parsed, Sheet};

/// `lxb_status_t`, the values the parser reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    ErrorMemoryAllocation,
    ErrorUnexpectedData,
    ErrorWrongArgs,
    ErrorOverflow,
}
