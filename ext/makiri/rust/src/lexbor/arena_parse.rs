//! Parsing into the arena DOM (`crate::html::dom`) while the tree builder is
//! still Lexbor's: the input is parsed by Lexbor exactly as for the Lexbor
//! DOM - sanitised, depth-guarded, every element stamped with its source
//! offset - and the tree is then copied into an arena document, which keeps
//! the line table. Stage 2 of `docs/rewrite_parser.ja.md`; the copy goes when
//! the tree builder is ported.

use crate::falloc::try_box;
use crate::html::dom::HtmlParsed;
use crate::lexbor::adapter::arena_copy::copy_document;
use crate::lexbor::adapter::post_parse::{self as lex, empty_html_document as lex_empty};
pub use crate::lexbor::adapter::post_parse::{parse_with_panicking_hook, HtmlParseError};
use crate::lexbor::adapter::tree_guard::DepthLimit;

/// Parse `src` as an HTML document into the arena - see the Lexbor
/// adapter's `parse_html`, whose parse this is.
pub fn parse_html(
    src: &[u8],
    assume_valid: bool,
    limit: DepthLimit,
) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let mut parsed = lex::parse_html(src, assume_valid, limit)?;
    let lines = parsed.take_lines().map(|b| *b);
    let doc = copy_document(parsed.doc_handle()).map_err(|_| HtmlParseError::Failed)?;
    drop(parsed);
    try_box(HtmlParsed::new(doc, lines)).map_err(|()| HtmlParseError::Failed)
}

/// A new HTML document with no children, in `compat_mode` - Lexbor's empty
/// document, copied.
pub fn empty_html_document(compat_mode: u32) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let parsed = lex_empty(compat_mode)?;
    let doc = copy_document(parsed.doc_handle()).map_err(|_| HtmlParseError::Failed)?;
    try_box(HtmlParsed::new(doc, None)).map_err(|()| HtmlParseError::Failed)
}
