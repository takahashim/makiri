//! Parsing into the arena DOM (`crate::html::dom`). The parse is the ported
//! tree builder's (`crate::html::tree`), straight into the arena; only the
//! `Makiri.__panic(6)` probe still parses with Lexbor, since what it proves is
//! the latch around Lexbor's tokenizer hook.

pub use crate::html::tree::{empty_html_document, parse_html, HtmlParseError};
pub use crate::lexbor::adapter::post_parse::parse_with_panicking_hook;
