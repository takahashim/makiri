//! The parse pipeline and the parsed-document lifecycle
//! (dom_adapter/post_parse.c).
//!
//! `parse_html` drives Lexbor's LOW-LEVEL pipeline rather than its one-shot
//! parse, because that is the only way to see the tokens: create and init a
//! parser, `chunk_begin`, override the tokenizer's token-done callback while
//! CHAINING the parser's own tree builder, then `chunk_process` and
//! `chunk_end`. The recorder that rides along is `dom_adapter::source_loc`.
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

use crate::cbuf::OwnedBuf;
use crate::dom_adapter::dom_index::DomIndex;
use crate::dom_adapter::source_loc::{
    lines_build, pos_assign_to_dom, pos_token_cb, Lines, Recorder,
};
use crate::dom_adapter::text_index::TextIndex;
pub use crate::dom_adapter::utf8_input::utf8_sanitize;
use crate::dom_adapter::utf8_input::Sanitized;
use crate::falloc::try_box;
use crate::lexbor_abi::{self as lxb, lxb_html_document_destroy, LxbDoc, LxbNode};
use crate::text::BorrowedText;
use crate::xml::model::Document as XmlDocument;

type HtmlDoc = lxb::lxb_html_document_t;

use super::html::TYPE_DOCUMENT as NODE_TYPE_DOCUMENT;
const LXB_STATUS_OK: u32 = lxb::lexbor_status_t_LXB_STATUS_OK;

extern "C" {

    fn lxb_html_parse_chunk_begin(parser: *mut lxb::lxb_html_parser_t) -> *mut HtmlDoc;
    fn lxb_html_parse_chunk_process(
        parser: *mut lxb::lxb_html_parser_t,
        data: *const u8,
        size: usize,
    ) -> u32;
    fn lxb_html_parse_chunk_end(parser: *mut lxb::lxb_html_parser_t) -> u32;
    fn lxb_dom_document_root(doc: *mut LxbDoc) -> *mut LxbNode;

}

/* ---- the parsed document ---- */

/// The document a parse produced.
enum Doc {
    /// A Lexbor document, destroyed with the handle.
    Html(NonNull<HtmlDoc>),
    /// An XML arena. None until the parse that fills it has finished, so a
    /// handle wrapped before that parse still frees cleanly.
    Xml(Option<Box<XmlDocument>>),
}

/// The result of a parse: the document, and the HTML indices built over it on
/// demand.
///
/// The indices point into the document's nodes and text, so every change to the
/// document goes through [`invalidate_indexes`](Parsed::invalidate_indexes),
/// which drops them; the next query rebuilds.
pub struct Parsed {
    doc: Doc,
    /// attr->owner map + the tag->elements index.
    dom_index: Option<Box<DomIndex>>,
    /// byte offset -> source line.
    lines: Option<Box<Lines>>,
    /// node -> descendant-text slice run.
    text_index: Option<Box<TextIndex>>,
    /// How many XPath evaluations that can run Ruby (ones with a handler)
    /// are reading this document right now. Every mutator refuses while it
    /// is non-zero - see `glue::doc::DocumentEvaluation`.
    pub evaluating: usize,
}

impl Drop for Parsed {
    fn drop(&mut self) {
        /* The indices point into the document, so they go first. */
        self.invalidate_indexes();
        if let Doc::Html(doc) = &self.doc {
            // SAFETY: the handle owns the document, and nothing reads it once
            // the handle is gone.
            unsafe { lxb_html_document_destroy(doc.as_ptr()) };
        }
    }
}

impl Parsed {
    fn with(doc: Doc) -> Parsed {
        Parsed {
            doc,
            dom_index: None,
            lines: None,
            text_index: None,
            evaluating: 0,
        }
    }

    /// A handle for an XML document still to be parsed. None on OOM.
    pub fn new_xml() -> Option<Box<Parsed>> {
        try_box(Parsed::with(Doc::Xml(None))).ok()
    }

    pub fn is_xml(&self) -> bool {
        matches!(self.doc, Doc::Xml(_))
    }

    /// The Lexbor document, or null for an XML handle.
    pub fn html_doc(&self) -> *mut HtmlDoc {
        match &self.doc {
            Doc::Html(doc) => doc.as_ptr(),
            Doc::Xml(_) => core::ptr::null_mut(),
        }
    }

    /// The XML arena, or null for an HTML handle or one not yet filled.
    pub fn xml_doc(&mut self) -> *mut XmlDocument {
        match &mut self.doc {
            Doc::Xml(Some(doc)) => &mut **doc,
            _ => core::ptr::null_mut(),
        }
    }

    /// The XML arena, for reading.
    pub fn xml_doc_ref(&self) -> Option<&XmlDocument> {
        match &self.doc {
            Doc::Xml(Some(doc)) => Some(doc),
            _ => None,
        }
    }

    /// Fill an XML handle with its parsed arena.
    pub fn set_xml_doc(&mut self, doc: Box<XmlDocument>) {
        debug_assert!(self.is_xml());
        self.doc = Doc::Xml(Some(doc));
    }

    /// The attr->owner and tag index, built on first use. None for an XML
    /// handle, or when the build cannot allocate - which caches nothing, so a
    /// later call retries.
    pub fn dom_index(&mut self) -> Option<&DomIndex> {
        let Doc::Html(doc) = &self.doc else {
            return None;
        };
        if self.dom_index.is_none() {
            // SAFETY: the handle owns a live document.
            let built =
                unsafe { crate::dom_adapter::dom_index::build(doc.as_ptr() as *mut LxbDoc) }?;
            self.dom_index = Some(try_box(built).ok()?);
        }
        self.dom_index.as_deref()
    }

    /// The run of text slices `node`'s subtree owns, and its byte total.
    ///
    /// None means "walk instead": a node outside the indexed tree (a
    /// fragment), an XML handle, or a build that could not allocate.
    pub fn text_slices(&mut self, node: *const LxbNode) -> Option<(&[BorrowedText], usize)> {
        let Doc::Html(doc) = &self.doc else {
            return None;
        };
        if self.text_index.is_none() {
            // SAFETY: the handle owns a live document.
            let root = unsafe { lxb_dom_document_root(doc.as_ptr() as *mut LxbDoc) };
            if root.is_null() {
                return None;
            }
            // SAFETY: `root` is the live document's root element.
            let built = unsafe { TextIndex::build(root) }?;
            self.text_index = Some(try_box(built).ok()?);
        }
        self.text_index.as_deref()?.slices_of(node)
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

    /// The 1-based source line for `node`, or 0 when unknown.
    ///
    /// 0 covers both "the tracker could not place this node" and "the line table
    /// could not be allocated". The two are deliberately not distinguished: the
    /// Ruby contract for `#line` is an Integer or nil, and the table's
    /// allocation is an allowed degradation - see `parse_tracked`.
    ///
    /// # Safety
    /// `node` must be null or a live node of this document.
    pub unsafe fn node_line(&self, node: *const LxbNode) -> usize {
        let Some(lines) = self.lines.as_deref() else {
            return 0;
        };
        if node.is_null() || (*node).user.is_null() {
            return 0;
        }
        lines.lookup((*node).user as usize - 1)
    }
}

/* ---- parsing ---- */

/// The sanitiser's replacement buffer, freed however the parse exits.
struct CleanBuf {
    _owned: Option<OwnedBuf>,
}

/// Drive the low-level pipeline so element offsets can be captured, then build
/// the line table.
///
/// Returns the document, or None on failure with everything it allocated
/// released, together with the line table - itself None if THAT allocation
/// failed, in which case line information degrades to nil rather than failing
/// the parse. That degradation is deliberate and is what
/// `spec/html_line_spec.rb`'s contract ("an Integer, or nil") allows.
unsafe fn parse_tracked(src: &[u8]) -> Option<(NonNull<HtmlDoc>, Option<Box<Lines>>)> {
    let parser = lxb::HtmlParser::create()?;

    let doc = NonNull::new(lxb_html_parse_chunk_begin(parser.as_ptr()))?;

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
    if st != LXB_STATUS_OK {
        lxb_html_document_destroy(doc.as_ptr());
        return None;
    }

    let mut lines = None;
    if let Some(r) = rec.take() {
        pos_assign_to_dom(&r, doc.as_ptr() as *mut LxbNode);
        /* Consumed: dropped now rather than at the end of the scope, because
         * the line table below is the last thing that needs the input. */
        drop(r);
        lines = lines_build(src).and_then(|l| try_box(l).ok());
    }

    Some((doc, lines))
}

/// Parse `src` as an HTML document.
///
/// `assume_valid` skips the UTF-8 validation scan entirely - the caller has
/// already proved the bytes valid, typically from a Ruby String's cached
/// coderange. None on failure.
///
/// # Safety
/// `src` must name `len` readable bytes, or be null with `len == 0`.
pub unsafe fn parse_html(src: *const u8, len: usize, assume_valid: bool) -> Option<Box<Parsed>> {
    if src.is_null() && len != 0 {
        return None;
    }

    /* Browser-compatible decoding: invalid UTF-8 becomes U+FFFD (WHATWG
     * byte-stream decoding), so parsing never fails on bad bytes and the DOM is
     * always valid UTF-8. Valid input - the common case - is used as-is with no
     * copy. Source offsets are then relative to the SANITISED bytes: exact for
     * valid input, best-effort where replacement shifted byte positions. */
    let mut clean = CleanBuf { _owned: None };
    if !assume_valid {
        match utf8_sanitize(src, len) {
            Some(Sanitized::Unchanged) => {}
            Some(Sanitized::Replaced(r)) => {
                clean._owned = Some(r);
            }
            None => return None, /* OOM */
        }
    }

    let bytes: &[u8] = if let Some(owned) = clean._owned.as_ref() {
        owned.as_slice()
    } else if src.is_null() {
        &[]
    } else {
        core::slice::from_raw_parts(src, len)
    };

    let (doc, lines) = parse_tracked(bytes)?;
    drop(clean); /* the parse is done with the buffer, on every path */

    let mut parsed = Parsed::with(Doc::Html(doc));
    parsed.lines = lines;
    /* On OOM the handle drops, and with it the document. */
    try_box(parsed).ok()
}

/* ---- live bytes, for sizing a serialization buffer ---- */

/// Bytes handed out from one Lexbor mem pool.
///
/// Lexbor exposes no running total, so the chunk list is walked summing each
/// chunk's bump length. Cheap: the chunks are few and large. Saturates to
/// `usize::MAX` on the unreachable overflow; the caller clamps the derived
/// capacity to the buffer's hard ceiling anyway.
unsafe fn mem_used(mem: *const lxb::lexbor_mem_t) -> usize {
    let mut total = 0usize;
    let mut c = if mem.is_null() {
        core::ptr::null_mut()
    } else {
        (*mem).chunk_first
    };
    while !c.is_null() {
        total = match total.checked_add((*c).length) {
            Some(t) => t,
            None => return usize::MAX,
        };
        c = (*c).next;
    }
    total
}

/// The live bytes in a node's document arena, which the serializers size their
/// buffer from.
pub unsafe fn lxb_document_bytes(node: *mut LxbNode) -> usize {
    if node.is_null() {
        return 0;
    }
    /* The document node owns itself; every other node points back through
     * owner_document. */
    let doc: *mut LxbDoc = if (*node).type_ == NODE_TYPE_DOCUMENT {
        node as *mut LxbDoc
    } else {
        (*node).owner_document
    };
    if doc.is_null() {
        return 0;
    }

    let mut total = 0usize;
    for pool in [(*doc).mraw, (*doc).text] {
        if pool.is_null() {
            continue;
        }
        total = match total.checked_add(mem_used((*pool).mem)) {
            Some(t) => t,
            None => return usize::MAX,
        };
    }
    total
}
