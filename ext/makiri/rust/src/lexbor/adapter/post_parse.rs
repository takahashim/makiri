//! The HTML parse pipeline and the parsed document it produces.
//!
//! `parse_html` drives Lexbor's LOW-LEVEL pipeline rather than its one-shot
//! parse, because that is the only way to see the tokens: create and init a
//! parser, `chunk_begin`, override the tokenizer's token-done callback while
//! CHAINING the parser's own tree builder, then `chunk_process` and
//! `chunk_end`. The hook is `lexbor::adapter::tree_guard`'s, which enforces
//! the tree-depth limit; the source-position stamper that rides in it is
//! `lexbor::adapter::source_loc`.
//!
//! Tracking is always on. The alternative that was tried - a separate source
//! scan - measured ~36% slower and was only approximate.
//!
//! # The document outlives the parser
//!
//! `lxb_html_parser_destroy` only unrefs the tokenizer and the tree, never the
//! document, which is what lets the parsed document be returned from a function
//! that destroys the parser on the way out.
//!
//! # Ordering the failure path
//!
//! Every resource is declared up front and released once, at a single exit.
//! The correct free-set then lives in one place instead of being re-spelled at
//! each early return - the shape the C used `goto fail` for, and which a Rust
//! `Drop` gives directly.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ptr::NonNull;

use crate::falloc::try_box;
use crate::lexbor::abi::{
    self as lxb, lxb_html_document_destroy, lxb_html_parse_chunk_begin, lxb_html_parse_chunk_end,
    lxb_html_parse_chunk_process,
};
use crate::lexbor::adapter::html::{HtmlDoc as DomDoc, RawDoc, RawNode};
use crate::lexbor::adapter::source_loc::{lines_build, Lines, Stamper};
use crate::lexbor::adapter::tree_guard::{ParseLimits, TokenHook};
use crate::utf8_input::sanitize;

type HtmlDoc = lxb::lxb_html_document_t;

use crate::lexbor::abi::consts::STATUS_OK as LXB_STATUS_OK;

/* ---- the parsed document ---- */

/// A document Lexbor parsed, as the reference the port is held to: its tree
/// and line table, read by the comparisons (`tree_diff`, the conformance
/// tests).
pub struct HtmlParsed {
    doc: NonNull<HtmlDoc>,
    /// byte offset -> source line.
    lines: Option<Box<Lines>>,
}

impl Drop for HtmlParsed {
    fn drop(&mut self) {
        // SAFETY: the handle owns the document, and nothing reads it once the
        // handle is gone.
        unsafe { lxb_html_document_destroy(self.doc.as_ptr()) };
    }
}

impl HtmlParsed {
    /// The document, as the handle that crosses to the Ruby glue.
    pub fn raw_doc(&self) -> RawDoc {
        RawDoc::from(self.doc())
    }

    /// The document as a handle, borrowed for as long as this is.
    pub fn doc(&self) -> DomDoc<'_> {
        // SAFETY: the handle owns a live document, and it is not restructured
        // while `&self` is borrowed - every mutation takes `&mut` of the
        // wrapper's content first (see `bridge::wrapper`).
        unsafe { DomDoc::from_non_null(self.doc.cast()) }
    }

    /// The line table, taken out - for the arena DOM, which copies this
    /// document and keeps the table.
    pub fn take_lines(&mut self) -> Option<Box<Lines>> {
        self.lines.take()
    }

    /// The 1-based source line for `node`, or `None` when unknown.
    ///
    /// `None` covers both "the tracker could not place this node" and "the line table
    /// could not be allocated". The two are deliberately not distinguished: the
    /// Ruby contract for `#line` is an Integer or nil, and the table's
    /// allocation is an allowed degradation - see `parse_tracked`.
    ///
    /// # Safety
    /// `node` must be a live node of this document.
    pub unsafe fn node_line(&self, node: RawNode) -> Option<usize> {
        let lines = self.lines.as_deref()?;
        // SAFETY: the caller's contract.
        let offset = unsafe { self.doc().node(node) }.source_offset()?;
        Some(lines.lookup(offset))
    }
}

/* ---- parsing ---- */

/// What a tracked parse produces: the document, its elements already stamped
/// with their offsets, and the line table - `None` when its allocation failed,
/// which degrades `#line` to nil rather than failing the parse.
type Tracked = (NonNull<HtmlDoc>, Option<Box<Lines>>);

/// Owns the document the parse is building, until it is handed to the caller.
///
/// Between `chunk_begin` and the return there is Rust that can panic - the
/// line table - and the crate unwinds, so an
/// explicit destroy on the failure path is exactly what a panic skips. This
/// frees the document on every exit that is not the hand-over.
struct DocOwner(NonNull<HtmlDoc>);

impl DocOwner {
    /// Give the document to the caller; this stops owning it.
    #[inline]
    fn release(self) -> NonNull<HtmlDoc> {
        let doc = self.0;
        core::mem::forget(self);
        doc
    }
}

impl Drop for DocOwner {
    fn drop(&mut self) {
        // SAFETY: this type owns the document - `release` is the only way out
        // and it consumes `self` - so nothing else holds it here.
        unsafe { lxb_html_document_destroy(self.0.as_ptr()) };
    }
}

/* The error is the ported parser's, so both parsers answer with one type. */
pub use crate::html::tree::HtmlParseError;

/// Drive the low-level pipeline so element offsets can be captured and the
/// tree depth bounded, then build the line table.
///
/// Returns the document, or the error with everything it allocated released,
/// together with the line table - itself None if THAT allocation failed, in
/// which case line information degrades to nil rather than failing the parse.
/// That degradation is deliberate and is what `spec/html_line_spec.rb`'s
/// contract ("an Integer, or nil") allows. The depth guard does not degrade:
/// it lives in the hook, which is a plain value installed on every parse, and
/// nothing the stamper does can switch it off (see `tree_guard`).
unsafe fn parse_tracked<const PANIC_PROBE: bool>(
    src: &[u8],
    limit: ParseLimits,
) -> Result<Tracked, HtmlParseError> {
    let parser = lxb::HtmlParser::create().ok_or(HtmlParseError::Failed)?;

    let doc = DocOwner(
        NonNull::new(lxb_html_parse_chunk_begin(parser.as_ptr())).ok_or(HtmlParseError::Failed)?,
    );

    /* Install the hook, CHAINING the parser's own tree-building callback. It is
     * declared after the parser, so it outlives nothing that can still call
     * it; it stays put until the parse calls below have returned. A document
     * keeps nothing below `<html>` on the open-element stack. */
    let mut hook = TokenHook::<PANIC_PROBE>::for_document(limit, Stamper::new(src));
    if !hook.install(parser.as_ptr()) {
        return Err(HtmlParseError::Failed); /* never unguarded */
    }

    let mut st = lxb_html_parse_chunk_process(parser.as_ptr(), src.as_ptr(), src.len());
    if st == LXB_STATUS_OK {
        st = lxb_html_parse_chunk_end(parser.as_ptr());
    }

    /* The tokenizer's callback cannot unwind into Lexbor, so a panic in the
     * hook was latched instead, and stopped the parse: `finish` raises it
     * here, before the status check. `doc`'s Drop and the parser's release it
     * all on the way out, and on a refusal. */
    hook.finish()?;
    if st != LXB_STATUS_OK {
        return Err(HtmlParseError::Failed);
    }

    /* The elements were stamped as the tree builder created them. The line
     * table is built here, eagerly - it is ~2%, and deferring it would mean
     * holding the source buffer, which is the one thing this function is about
     * to free. */
    let lines = lines_build(src).and_then(|l| try_box(l).ok());

    Ok((doc.release(), lines))
}

/// Parse `src` as an HTML document, refusing a tree deeper than `limit`.
///
/// Browser-compatible decoding first: invalid UTF-8 becomes U+FFFD (WHATWG
/// byte-stream decoding), so parsing never fails on bad bytes and the DOM is
/// always valid UTF-8. `assume_valid` skips that scan - the caller has already
/// proved the bytes valid. Source offsets are relative to the SANITISED bytes:
/// exact for valid input, best-effort where replacement shifted positions.
pub fn parse_html(
    src: &[u8],
    assume_valid: bool,
    limit: ParseLimits,
) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let input = sanitize(src, assume_valid).ok_or(HtmlParseError::Failed)?;
    // SAFETY: a live slice, which the parse only reads and is done with when it
    // returns.
    let (doc, lines) = unsafe { parse_tracked::<false>(input.as_slice(), limit) }?;
    drop(input); /* the parse is done with the buffer, on every path */

    let parsed = HtmlParsed { doc, lines };
    /* On OOM the handle drops, and with it the document. */
    try_box(parsed).map_err(|()| HtmlParseError::Failed)
}

/// A new HTML document with no children, in `compat_mode` (Lexbor's: 0
/// no-quirks, 1 quirks, 2 limited-quirks) - what `Makiri::HTML::Document.new`
/// holds (no-quirks, as the DOM's `createHTMLDocument` starts from before it
/// adds its skeleton), and what `Document#dup` copies into (the source's
/// mode). Made by Lexbor's document constructor, not a parse, so nothing is
/// stamped and there is no line table.
pub fn empty_html_document(compat_mode: u32) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let doc = DocOwner(DomDoc::create_empty(compat_mode).ok_or(HtmlParseError::Failed)?);
    let parsed = HtmlParsed {
        doc: doc.release(),
        lines: None,
    };
    /* On OOM the handle drops, and with it the document. */
    try_box(parsed).map_err(|()| HtmlParseError::Failed)
}

/// `Makiri.__panic(6)`'s parse: a real document parse whose token hook panics
/// on the first token, inside Lexbor's tokenizer. The latch stops the parse and
/// this re-raises the panic once Lexbor has returned, so it never returns
/// normally; the result exists only so the type says nothing leaks if it did.
pub fn parse_with_panicking_hook() -> bool {
    // SAFETY: a static slice, which the parse only reads.
    let parsed = unsafe { parse_tracked::<true>(b"<p>x</p>", ParseLimits::DEFAULT) };
    parsed.map(|(doc, _)| drop(DocOwner(doc))).is_ok()
}
