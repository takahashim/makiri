//! Parsing into the arena DOM (`crate::html::dom`). The parse is the ported
//! tree builder's (`crate::html::tree`), straight into the arena, and so is
//! the `Makiri.__panic(6)` probe's: there is no C frame left between the
//! tokenizer and the tree builder for its panic to cross.

pub use crate::html::tree::{
    empty_html_document, parse_html, parse_with_panicking_hook, HtmlParseError,
};
