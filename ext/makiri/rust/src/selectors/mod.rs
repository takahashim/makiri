//! The CSS selector parser, ported from Lexbor's `css/syntax` (the tokenizer
//! and the rule machinery selectors run on) and `css/selectors` (the selector
//! states), producing an owned AST.
//!
//! Ruby- and Lexbor-free, like the HTML parser port in `crate::html`: Lexbor's
//! own parser is kept only as the reference the tests compare against.

#![forbid(unsafe_code)]

pub mod ast;
pub mod parser;
pub mod pseudo;
mod strtod;
pub mod token;
pub mod tokenizer;

pub use parser::{parse, Parsed};

/// `lxb_status_t`, the values the parser reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    ErrorMemoryAllocation,
    ErrorUnexpectedData,
    ErrorWrongArgs,
    ErrorOverflow,
}
