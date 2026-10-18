//! Parsing a whole document into a new arena document: Lexbor's
//! `lxb_html_parse_chunk_begin` / `_process` / `_end` with the parse guard
//! and the source stamps of `lexbor::adapter::post_parse`, as one Rust call.

use super::guard::{DepthLimit, Guard, GuardStop};
use super::{TreeBuilder, TreeError, TreeStatus};
use crate::falloc::try_box;
use crate::html::dom::{Document, DomOom, HtmlParsed, NodeId};
use crate::html::res::names as res;
use crate::html::tokenizer::{Status, Tokenizer};
use crate::source_lines::lines_build;
use crate::utf8_input::sanitize;

/// Why an HTML document parse produced no document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlParseError {
    /// Out of memory, or the parser refused the input.
    Failed,
    /// The parse guard refused it ([`GuardStop`]), as it refuses a fragment.
    Guard(GuardStop),
}

impl From<GuardStop> for HtmlParseError {
    fn from(stop: GuardStop) -> Self {
        HtmlParseError::Guard(stop)
    }
}

/// Why [`parse_document`] produced no document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseFailure {
    /// An allocation failed.
    Oom,
    /// The tree builder stopped the parse.
    Stopped(TreeStatus),
    /// The guard refused it.
    Guard(GuardStop),
}

impl From<ParseFailure> for HtmlParseError {
    fn from(f: ParseFailure) -> Self {
        match f {
            ParseFailure::Guard(stop) => HtmlParseError::Guard(stop),
            ParseFailure::Oom | ParseFailure::Stopped(_) => HtmlParseError::Failed,
        }
    }
}

/// A parsed document and the tree builder's parse errors.
pub struct Parsed {
    pub doc: Document,
    pub errors: Vec<TreeError>,
}

/// `lxb_html_document_parse`, unguarded and unstamped: tokenize `input` -
/// valid UTF-8, the whole document - and build its tree.
pub fn parse_document(input: &[u8]) -> Result<Parsed, ParseFailure> {
    parse_with(input, Guard::default())
}

/// [`parse_document`] under `guard`.
fn parse_with(input: &[u8], guard: Guard) -> Result<Parsed, ParseFailure> {
    let mut doc = Document::new(0).map_err(|DomOom| ParseFailure::Oom)?;
    /* An HTML document node is in the HTML namespace
     * (`lxb_dom_document_init`). */
    doc.set_names(NodeId::DOCUMENT, res::TAG__DOCUMENT, res::NS_HTML as u32, 0);
    let mut tree = TreeBuilder::new(&mut doc);
    tree.guard = guard;
    let status = {
        let mut tkz = Tokenizer::new(input, &mut tree);
        /* `lxb_html_parser_init` */
        tkz.keep_duplicate = true;
        match tkz.chunk() {
            Status::Ok => tkz.end(),
            st => st,
        }
    };
    if let Some(stop) = tree.guard.stopped {
        return Err(ParseFailure::Guard(stop));
    }
    if status != Status::Ok {
        return Err(match status {
            Status::Oom => ParseFailure::Oom,
            _ => ParseFailure::Stopped(tree.status),
        });
    }
    /* `lxb_html_parse_chunk_end`: whatever is still open is popped, with
     * its pop steps. */
    tree.open_pop_all();
    if tree.status != TreeStatus::Ok {
        return Err(ParseFailure::Stopped(tree.status));
    }
    let errors = core::mem::take(&mut tree.errors);
    drop(tree);
    Ok(Parsed { doc, errors })
}

/// Parse `src` as an HTML document into the arena, refusing a tree deeper
/// than `limit` - `lexbor::adapter::post_parse::parse_html`, ported:
/// invalid UTF-8 becomes U+FFFD first (unless `assume_valid`), every element
/// made from a start tag carries that tag's offset, and the line table is
/// built - `None` when it could not be, which degrades `#line` to nil.
pub fn parse_html(
    src: &[u8],
    assume_valid: bool,
    limit: DepthLimit,
) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let input = sanitize(src, assume_valid).ok_or(HtmlParseError::Failed)?;
    let bytes = input.as_slice();
    let parsed = parse_with(bytes, Guard::for_document(limit, bytes.len()))?;
    let lines = lines_build(bytes);
    drop(input);
    try_box(HtmlParsed::new(parsed.doc, lines)).map_err(|()| HtmlParseError::Failed)
}

/// A new HTML document with no children, in `compat_mode` (0 no-quirks, 1
/// quirks, 2 limited-quirks) - what Lexbor's document constructor makes.
pub fn empty_html_document(compat_mode: u32) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let mode = u8::try_from(compat_mode).unwrap_or(2);
    let mut doc = Document::new(mode).map_err(|DomOom| HtmlParseError::Failed)?;
    doc.set_names(NodeId::DOCUMENT, res::TAG__DOCUMENT, res::NS_HTML as u32, 0);
    try_box(HtmlParsed::new(doc, None)).map_err(|()| HtmlParseError::Failed)
}
