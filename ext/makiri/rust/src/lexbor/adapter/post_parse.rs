//! The HTML parse pipeline and the parsed document it produces.
//!
//! `parse_html` drives Lexbor's LOW-LEVEL pipeline rather than its one-shot
//! parse, because that is the only way to see the tokens: create and init a
//! parser, `chunk_begin`, override the tokenizer's token-done callback while
//! CHAINING the parser's own tree builder, then `chunk_process` and
//! `chunk_end`. The recorder that rides along is `lexbor::adapter::source_loc`.
//!
//! Tracking is always on: it costs about 7% over no-tracking, measured, and the
//! alternative that was tried - a separate source scan - measured ~36% slower
//! and was only approximate.
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

use core::ffi::c_void;
use core::ptr::NonNull;

use crate::falloc::try_box;
use crate::lexbor::abi::{
    self as lxb, lxb_html_document_destroy, lxb_html_parse_chunk_begin, lxb_html_parse_chunk_end,
    lxb_html_parse_chunk_process,
};
use crate::lexbor::adapter::arena_bytes::document_capacity;
use crate::lexbor::adapter::dom_index::DomIndex;
use crate::lexbor::adapter::html::{HtmlDoc as DomDoc, HtmlNode, RawDoc, RawNode, TagId};
use crate::lexbor::adapter::source_loc::{
    lines_build, pos_assign_to_dom, pos_token_cb, Lines, Positions, Recorder,
};
use crate::lexbor::adapter::text_index::{TextIndex, TextRun};
use crate::utf8_input::sanitize;

type HtmlDoc = lxb::lxb_html_document_t;

use crate::lexbor::abi::consts::STATUS_OK as LXB_STATUS_OK;

/* ---- the parsed document ---- */

/// The element index could not be built: an allocation failed. Nothing is
/// cached, so the next [`HtmlParsed::ensure_dom_index`] tries again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexOom;

/// A parsed HTML document, and the indices built over it on demand.
///
/// The indices point into the document's nodes and text, so every change to the
/// document goes through [`invalidate_indexes`](HtmlParsed::invalidate_indexes),
/// which drops them; the next query rebuilds.
///
/// HTML only. What a Ruby Document owns - this or an XML arena, plus the count
/// of evaluations reading it - is `bridge::wrapper`'s: the XML engine's
/// document does not belong to the Lexbor adapter, and the evaluation count is
/// the Ruby layer's mutation gate.
pub struct HtmlParsed {
    doc: NonNull<HtmlDoc>,
    /// The tag -> elements index (`//tag`).
    dom_index: Option<Box<DomIndex>>,
    /// byte offset -> source line.
    lines: Option<Box<Lines>>,
    /// Recorded element offsets, NOT yet stamped into the DOM.
    ///
    /// The stamping walks the whole tree, and it measured 11% of a parse - paid
    /// by every caller, for a `#line` most never ask for. So the parse records
    /// and stops; [`assign_positions`](HtmlParsed::assign_positions) does the
    /// walk on the first `#line`, or on the first MUTATION, whichever comes
    /// first. The second is what keeps the answers identical to stamping
    /// eagerly: a walk over an edited tree would match elements to the wrong
    /// tokens, and a wrong line is the one thing `#line` must never give.
    pending_pos: Option<Box<Positions>>,
    /// node -> descendant-text slice run.
    text_index: Option<Box<TextIndex>>,
}

impl Drop for HtmlParsed {
    fn drop(&mut self) {
        /* The indices point into the document, so they go first. */
        self.invalidate_indexes();
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
    fn doc(&self) -> DomDoc<'_> {
        // SAFETY: the handle owns a live document, and it is not restructured
        // while `&self` is borrowed - every mutation takes `&mut` of the
        // wrapper's content first (see `bridge::wrapper`).
        unsafe { DomDoc::from_non_null(self.doc.cast()) }
    }

    /// Build the tag -> elements index if a mutation (or nothing yet) left it
    /// unbuilt. `Err` when the build cannot allocate - which caches nothing,
    /// so a later call retries.
    ///
    /// The one writer; the readers below take `&self`, so a reader holding
    /// what they lend cannot be overlapped by a rebuild.
    pub fn ensure_dom_index(&mut self) -> Result<(), IndexOom> {
        if self.dom_index.is_none() {
            let built = crate::lexbor::adapter::dom_index::build(self.doc()).ok_or(IndexOom)?;
            self.dom_index = Some(try_box(built).map_err(|()| IndexOom)?);
        }
        Ok(())
    }

    /// The tag -> elements index, or `None` until
    /// [`ensure_dom_index`](Self::ensure_dom_index) has built it.
    pub fn dom_index(&self) -> Option<&DomIndex> {
        self.dom_index.as_deref()
    }

    /// The elements with tag id `tag`, in document order, as typed nodes
    /// borrowed from this handle. `None` until
    /// [`ensure_dom_index`](Self::ensure_dom_index) has built the index.
    ///
    /// Safe, and bounded by `&self`, because this handle is what makes the
    /// nodes live: the index is built only from `self`'s own document, is
    /// dropped with it, and is dropped by [`invalidate_indexes`] - which takes
    /// `&mut self` - before any edit. An edit that skipped it would first have
    /// had to break the contract of the `unsafe` mutable handle
    /// (`HtmlNodeMut::assume_mutable`).
    ///
    /// [`invalidate_indexes`]: Self::invalidate_indexes
    pub fn tag_bucket(&self, tag: TagId) -> Option<&[HtmlNode<'_>]> {
        let index = self.dom_index.as_deref()?;
        // SAFETY: every node of the bucket is a live element of `self.doc`, and
        // stays one for as long as `self` is borrowed - see above. The
        // lifetime is `&self`'s, not one of the caller's choosing.
        Some(unsafe { RawNode::as_html_nodes_unchecked(index.tag_bucket(tag)) })
    }

    /// The run of text slices `node`'s subtree owns, and its byte total.
    ///
    /// None means "walk instead": a node outside the indexed tree (a
    /// fragment), or a build that could not allocate.
    pub fn text_slices(&mut self, node: RawNode) -> Option<TextRun<'_>> {
        if self.text_index.is_none() {
            let root = self.doc().as_node().document_root()?;
            let built = TextIndex::build(root)?;
            self.text_index = Some(try_box(built).ok()?);
        }
        self.text_index.as_deref()?.slices_of(node)
    }

    /// Stamp the recorded offsets into the DOM, once.
    ///
    /// A no-op after the first call, and for a document that recorded nothing.
    /// Both callers are deliberate: `node_line`, which needs the answer, and
    /// the mutation gate, which needs the tree to still be the parsed one.
    ///
    /// # Safety
    /// The document must be unmodified since the parse.
    pub unsafe fn assign_positions(&mut self) {
        if let Some(pos) = self.pending_pos.take() {
            /* `pos` is this parse's own recording - taken above, so this runs
             * once. */
            pos_assign_to_dom(&pos, self.doc().as_node());
        }
    }

    /// Drop the indices so the next query rebuilds them.
    ///
    /// This is the whole safety protocol for what they borrow: EVERY mutation
    /// reaches here, so no cached node or text slice outlives the storage it
    /// points into.
    pub fn invalidate_indexes(&mut self) {
        self.dom_index = None;
        self.text_index = None;
    }

    /// The bytes this document holds OUTSIDE Ruby's allocator, for the GC:
    /// arena CAPACITY, not the bytes in use, because the pages are what cost.
    pub fn external_bytes(&self) -> usize {
        document_capacity(self.doc())
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
    pub unsafe fn node_line(&mut self, node: RawNode) -> Option<usize> {
        // SAFETY: the document this handle owns, unchanged since the parse -
        // any mutation would have stamped already (see `assign_positions`).
        unsafe { self.assign_positions() };
        let lines = self.lines.as_deref()?;
        // SAFETY: the caller's contract.
        let offset = unsafe { node.as_node() }.source_offset()?;
        Some(lines.lookup(offset))
    }
}

/* ---- parsing ---- */

/// What a tracked parse produces: the document, the line table, and the element
/// offsets still to be stamped into it. The last two are `None` when their
/// allocation failed, which degrades `#line` to nil rather than failing the
/// parse.
type Tracked = (NonNull<HtmlDoc>, Option<Box<Lines>>, Option<Box<Positions>>);

/// Owns the document the parse is building, until it is handed to the caller.
///
/// Between `chunk_begin` and the return there is Rust that can panic - the
/// source-position walk and the line table - and the crate unwinds, so an
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

/// Drive the low-level pipeline so element offsets can be captured, then build
/// the line table.
///
/// Returns the document, or None on failure with everything it allocated
/// released, together with the line table - itself None if THAT allocation
/// failed, in which case line information degrades to nil rather than failing
/// the parse. That degradation is deliberate and is what
/// `spec/html_line_spec.rb`'s contract ("an Integer, or nil") allows.
unsafe fn parse_tracked(src: &[u8]) -> Option<Tracked> {
    let parser = lxb::HtmlParser::create()?;

    let doc = DocOwner(NonNull::new(lxb_html_parse_chunk_begin(parser.as_ptr()))?);

    /* Install the recorder, CHAINING the parser's own tree-building callback
     * (which chunk_begin has just set). If the recorder cannot be allocated we
     * simply parse without source tracking. It is declared after the parser,
     * so it outlives nothing that can still call it. */
    let mut rec = try_box(Recorder::new(src.as_ptr())).ok();
    if let Some(r) = rec.as_deref_mut() {
        let tkz = lxb::lxb_html_parser_tokenizer_noi(parser.as_ptr());
        /* Lexbor has a setter and a ctx getter for the token-done callback but
         * no getter for the callback FUNCTION, so that one field is read from
         * the struct directly; the ctx uses the public accessor. */
        r.set_delegate(
            (*tkz).callback_token_done,
            lxb::lxb_html_tokenizer_callback_token_done_ctx_noi(tkz),
        );
        lxb::lxb_html_tokenizer_callback_token_done_set_noi(
            tkz,
            Some(pos_token_cb),
            r as *mut Recorder as *mut c_void,
        );
    }

    let mut st = lxb_html_parse_chunk_process(parser.as_ptr(), src.as_ptr(), src.len());
    if st == LXB_STATUS_OK {
        st = lxb_html_parse_chunk_end(parser.as_ptr());
    }

    /* The tokenizer's callback cannot unwind into Lexbor, so a panic in the
     * position recorder was latched instead. Lexbor has returned, so this is
     * the first frame where raising it is safe - and it raises BEFORE the
     * status check, because a panic is not a parse failure. `doc`'s Drop and
     * the parser's release it all on the way out. */
    if let Some(r) = rec.as_deref_mut() {
        r.resume_panic();
    }

    if st != LXB_STATUS_OK {
        return None; /* `doc`'s Drop destroys it */
    }

    /* The recording is HANDED BACK rather than stamped here: the stamping walks
     * the whole tree, which measured 11% of a parse, and most callers never ask
     * for a line. `HtmlParsed::assign_positions` does it on demand. The line table
     * stays eager - it is ~2%, and deferring it would mean holding the source
     * buffer, which is the one thing this function is about to free. */
    let mut lines = None;
    let mut positions = None;
    if let Some(r) = rec.take() {
        positions = try_box(r.into_positions()).ok();
        lines = lines_build(src).and_then(|l| try_box(l).ok());
    }

    Some((doc.release(), lines, positions))
}

/// Parse `src` as an HTML document.
///
/// Browser-compatible decoding first: invalid UTF-8 becomes U+FFFD (WHATWG
/// byte-stream decoding), so parsing never fails on bad bytes and the DOM is
/// always valid UTF-8. `assume_valid` skips that scan - the caller has already
/// proved the bytes valid. Source offsets are relative to the SANITISED bytes:
/// exact for valid input, best-effort where replacement shifted positions.
/// None on failure.
pub fn parse_html(src: &[u8], assume_valid: bool) -> Option<Box<HtmlParsed>> {
    let input = sanitize(src, assume_valid)?;
    // SAFETY: a live slice, which the parse only reads and is done with when it
    // returns.
    let (doc, lines, positions) = unsafe { parse_tracked(input.as_slice()) }?;
    drop(input); /* the parse is done with the buffer, on every path */

    let parsed = HtmlParsed {
        doc,
        dom_index: None,
        lines,
        pending_pos: positions,
        text_index: None,
    };
    /* On OOM the handle drops, and with it the document. */
    try_box(parsed).ok()
}
