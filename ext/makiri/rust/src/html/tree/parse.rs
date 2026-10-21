//! Parsing a whole document into a new arena document: Lexbor's
//! `lxb_html_parse_chunk_begin` / `_process` / `_end` with the parse guard
//! and the source stamps of `lexbor::adapter::post_parse`, as one Rust call.

use super::guard::{DepthLimit, Guard, GuardStop};
use super::stamp::Stamping;
use super::{TreeBuilder, TreeError, TreeStatus};
use crate::falloc::try_box;
use crate::html::dom::{Document, DomOom, HtmlParsed};
use crate::html::names::Names;
use crate::html::tokenizer::{Options, Status, Stop, TokenSink, Tokenizer};
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
    parse_with(input, Guard::default(), Stamping::default())
}

/// Tokenize `input` into `tree`, one chunk and then the end, as Lexbor's
/// parser sets its tokenizer up (`lxb_html_parser_init` keeps duplicate
/// attributes); `setup` adjusts the tokenizer before the first chunk.
pub(super) fn drive(
    tree: &mut TreeBuilder<'_>,
    input: &[u8],
    undef_is_foreign: bool,
    setup: impl FnOnce(&mut Tokenizer<'_, '_>),
) -> Status {
    let options = Options {
        keep_duplicate: true,
        undef_is_foreign,
    };
    let mut tkz = Tokenizer::with_options(input, tree, options);
    setup(&mut tkz);
    match tkz.chunk() {
        Status::Ok => tkz.end(),
        st => st,
    }
}

/// [`parse_document`] under `guard`, stamping as `stamping` says.
fn parse_with(input: &[u8], guard: Guard, stamping: Stamping) -> Result<Parsed, ParseFailure> {
    let mut doc = Document::new(0).map_err(|DomOom| ParseFailure::Oom)?;
    let mut tree = TreeBuilder::for_document(&mut doc, guard, stamping);
    let status = drive(&mut tree, input, false, |_| {});
    if let Some(stop) = tree.guard_stopped() {
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
    /* What the parse left dead - the rooms `append_text` reserved - goes now
     * when it outweighs the live bytes, rather than at the first edit, which a
     * document only read never makes. A failed compaction leaves the store as
     * it is. */
    if doc.compaction_due() {
        let _ = doc.compact();
    }
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
    let parsed = parse_with(
        bytes,
        Guard::for_document(limit),
        Stamping::document(bytes.len()),
    )?;
    let lines = lines_build(bytes);
    drop(input);
    try_box(HtmlParsed::new(parsed.doc, lines)).map_err(|()| HtmlParseError::Failed)
}

/// A new HTML document with no children, in `compat_mode` (0 no-quirks, 1
/// quirks, 2 limited-quirks) - what Lexbor's document constructor makes.
pub fn empty_html_document(compat_mode: u32) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let mode = u8::try_from(compat_mode).unwrap_or(2);
    let doc = Document::new(mode).map_err(|DomOom| HtmlParseError::Failed)?;
    try_box(HtmlParsed::new(doc, None)).map_err(|()| HtmlParseError::Failed)
}

/// `Makiri.__panic(6)`'s parse: a real document parse that panics in the
/// token-done path, right after the tree builder has taken the first token.
/// It never returns; the panic unwinds through the tokenizer and the tree
/// builder, which hold nothing a skipped statement would leak, to the
/// caller's `entry`.
pub fn parse_with_panicking_hook() -> bool {
    struct Panicking<'t, 'd>(&'t mut TreeBuilder<'d>);
    impl TokenSink for Panicking<'_, '_> {
        fn token(&mut self, tkz: &mut Tokenizer<'_, '_>) -> Result<(), Stop> {
            self.0.token(tkz)?;
            #[allow(clippy::panic, reason = "the probe's whole purpose")]
            {
                panic!("Makiri.__panic(6): panic inside the tokenizer hook");
            }
        }
        fn names(&mut self) -> &mut Names {
            self.0.names()
        }
    }
    let Ok(mut doc) = Document::new(0) else {
        return false;
    };
    let mut tree = TreeBuilder::for_document(&mut doc, Guard::default(), Stamping::default());
    let mut sink = Panicking(&mut tree);
    let mut tkz = Tokenizer::new(b"<p>x</p>", &mut sink);
    tkz.chunk() == Status::Ok
}
