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

use crate::falloc::{try_box, OomOption, OomResult};
use crate::lexbor::abi::{
    self as lxb, lxb_html_document_create, lxb_html_document_destroy, lxb_html_parse_chunk_begin,
    lxb_html_parse_chunk_end, lxb_html_parse_chunk_process,
};
use crate::lexbor::adapter::arena_bytes::{document_capacity, document_chunks};
use crate::lexbor::adapter::dom_index::DomIndex;
use crate::lexbor::adapter::html::{
    ForeignNode, HtmlDoc as DomDoc, HtmlDocIdentity, HtmlNode, HtmlNodeKey, RawDoc, RawNode, TagId,
};
use crate::lexbor::adapter::source_loc::{lines_build, Lines, Stamper};
use crate::lexbor::adapter::text_index::{TextBuildError, TextIndex, TextRun};
use crate::lexbor::adapter::tree_guard::{DepthLimit, GuardStop, TokenHook};
use crate::utf8_input::sanitize;

type HtmlDoc = lxb::lxb_html_document_t;

use crate::lexbor::abi::consts::STATUS_OK as LXB_STATUS_OK;

/* ---- the parsed document ---- */

use super::AdapterOom;

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
    /// node -> descendant-text slice run.
    text_index: TextIndexState,
}

/// The text index's state, so a document the index cannot serve does not rebuild
/// on every `Node#text`.
enum TextIndexState {
    /// Not built since the last invalidation.
    Unbuilt,
    /// Built over the document root.
    Built(Box<TextIndex>),
    /// The index does not fit this document (its root is not a container, or it
    /// holds more slices than a run can index). Retrying cannot change that.
    Inapplicable,
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
    pub fn ensure_dom_index(&mut self) -> Result<(), AdapterOom> {
        if self.dom_index.is_none() {
            let built = crate::lexbor::adapter::dom_index::build(self.doc()).or_oom()?;
            self.dom_index = Some(try_box(built).or_oom()?);
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
    /// [`ensure_dom_index`](Self::ensure_dom_index) has built the index, and
    /// for a tag the index does not bucket (see [`DomIndex::tag_bucket`]).
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
        let bucket = self.dom_index.as_deref()?.tag_bucket(tag)?;
        // SAFETY: every node of the bucket is a live element of `self.doc`, and
        // stays one for as long as `self` is borrowed - see above. The
        // lifetime is `&self`'s, not one of the caller's choosing.
        Some(unsafe { RawNode::as_html_nodes_unchecked(bucket) })
    }

    /// Build the node -> text index if it is not built and the document can
    /// have one. `Err` when the build could not allocate - which caches
    /// nothing, so a later call retries; an index that does not fit this
    /// document is remembered so it is never attempted again.
    pub fn ensure_text_index(&mut self) -> Result<(), AdapterOom> {
        if matches!(self.text_index, TextIndexState::Unbuilt) {
            let Some(root) = self.doc().as_node().document_root() else {
                self.text_index = TextIndexState::Inapplicable;
                return Ok(());
            };
            match TextIndex::build(root) {
                Ok(built) => {
                    self.text_index = TextIndexState::Built(try_box(built).or_oom()?);
                }
                Err(TextBuildError::NotApplicable) => {
                    self.text_index = TextIndexState::Inapplicable;
                }
                Err(TextBuildError::Oom) => return Err(AdapterOom),
            }
        }
        Ok(())
    }

    /// The run of text slices `node`'s subtree owns, and its byte total.
    ///
    /// None means "walk instead": the index is not built (call
    /// [`ensure_text_index`](Self::ensure_text_index) first), does not fit this
    /// document, or does not place this node (a fragment outside the tree).
    pub fn text_slices(&self, node: RawNode) -> Option<TextRun<'_>> {
        match &self.text_index {
            TextIndexState::Built(index) => index.slices_of(node),
            TextIndexState::Unbuilt | TextIndexState::Inapplicable => None,
        }
    }

    /// Drop the indices so the next query rebuilds them.
    ///
    /// This is the whole safety protocol for what they borrow: EVERY mutation
    /// reaches here, so no cached node or text slice outlives the storage it
    /// points into.
    pub fn invalidate_indexes(&mut self) {
        self.dom_index = None;
        self.text_index = TextIndexState::Unbuilt;
    }

    /// The bytes this document holds OUTSIDE Ruby's allocator, for the GC:
    /// arena CAPACITY, not the bytes in use, because the pages are what cost.
    pub fn external_bytes(&self) -> usize {
        document_capacity(self.doc())
    }

    /// How many chunks the document's pools hold: an O(1) stand-in for
    /// [`external_bytes`](Self::external_bytes), for deciding when to measure
    /// it again.
    pub fn arena_chunks(&self) -> usize {
        document_chunks(self.doc())
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
        let offset = unsafe { node.as_node() }.source_offset()?;
        Some(lines.lookup(offset))
    }

    /* ---- long-lived node handles ---- */

    /// The identity of this document, for a long-lived node key. See
    /// `HtmlDocIdentity`: the stable address of this handle.
    #[cfg_attr(not(feature = "ruby"), allow(dead_code))]
    #[inline]
    pub(crate) fn identity(&self) -> HtmlDocIdentity {
        HtmlDocIdentity::of(self)
    }

    /// The key for `node`, which must be a node this document owns.
    ///
    /// `Err(ForeignNode)` for a live node of another document. This is the one
    /// checked producer of keys: the safe `resolve` trusts every key to have
    /// come from here (or from `HtmlNodeKey::new` under its contract).
    ///
    /// # Safety
    /// `node` must be a live node - the contract [`RawNode::as_node`] states,
    /// which this function reads the node's owner document through - AND the
    /// caller must keep THIS document alive at the same address until the key
    /// is no longer used. A key is `Copy`, so it can outlive the document;
    /// `resolve` compares the owner address and then dereferences the node, so
    /// a reused address would pass the comparison and dereference a stale
    /// pointer. The Ruby path keeps the document alive by marking it from the
    /// wrapper that holds the key.
    #[cfg_attr(not(feature = "ruby"), allow(dead_code))]
    pub(crate) unsafe fn mint_key(&self, node: RawNode) -> Result<HtmlNodeKey, ForeignNode> {
        // SAFETY: the caller's contract - `node` is live.
        let owner = unsafe { node.as_node() }.owner_document();
        if owner.as_raw() != self.doc().as_raw() {
            return Err(ForeignNode);
        }
        // SAFETY: `node` is live (the caller's contract) and its owner is this
        // document (just checked).
        Ok(unsafe { HtmlNodeKey::new(self.identity(), node) })
    }

    /// The node `key` names, when it belongs to this document.
    ///
    /// The owner identity is compared BEFORE the node pointer is touched, so a
    /// key minted under another document is refused rather than dereferenced.
    /// Sound because a key exists only through `mint_key`, or through
    /// `HtmlNodeKey::new` under its own (unsafe) contract, and both require a
    /// live node of the owner document - which the caller must have kept alive
    /// at its address until this call (see `mint_key`'s contract).
    #[cfg_attr(not(feature = "ruby"), allow(dead_code))]
    pub(crate) fn resolve(&self, key: HtmlNodeKey) -> Result<HtmlNode<'_>, ForeignNode> {
        if key.owner() != self.identity() {
            return Err(ForeignNode);
        }
        // SAFETY: a key's node is a live node of the document its owner names -
        // the key's construction contract - and `self` is that document.
        Ok(unsafe { key.raw_node().as_node() })
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

/// Why an HTML document parse produced no document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlParseError {
    /// Out of memory, or Lexbor refused the input.
    Failed,
    /// The tree grew deeper than the [`DepthLimit`] allowed.
    TooDeep,
    /// A `<select>` received more than `MAX_SELECT_OPTIONS` options.
    TooManyOptions,
}

impl From<GuardStop> for HtmlParseError {
    fn from(stop: GuardStop) -> Self {
        match stop {
            GuardStop::TooDeep => HtmlParseError::TooDeep,
            GuardStop::TooManyOptions => HtmlParseError::TooManyOptions,
        }
    }
}

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
    limit: DepthLimit,
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
    limit: DepthLimit,
) -> Result<Box<HtmlParsed>, HtmlParseError> {
    let input = sanitize(src, assume_valid).ok_or(HtmlParseError::Failed)?;
    // SAFETY: a live slice, which the parse only reads and is done with when it
    // returns.
    let (doc, lines) = unsafe { parse_tracked::<false>(input.as_slice(), limit) }?;
    drop(input); /* the parse is done with the buffer, on every path */

    let parsed = HtmlParsed {
        doc,
        dom_index: None,
        lines,
        text_index: TextIndexState::Unbuilt,
    };
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
    // SAFETY: the constructor takes nothing and returns a new document, or
    // null when it cannot allocate one; a non-null one is ours to write.
    let raw = unsafe {
        let raw = lxb_html_document_create();
        if !raw.is_null() {
            (*raw).dom_document.compat_mode = compat_mode.min(2);
        }
        raw
    };
    let doc = DocOwner(NonNull::new(raw).ok_or(HtmlParseError::Failed)?);
    let parsed = HtmlParsed {
        doc: doc.release(),
        dom_index: None,
        lines: None,
        text_index: TextIndexState::Unbuilt,
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
    let parsed = unsafe { parse_tracked::<true>(b"<p>x</p>", DepthLimit::DEFAULT) };
    parsed.map(|(doc, _)| drop(DocOwner(doc))).is_ok()
}
