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

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::cbuf::OwnedBuf;
use crate::falloc::try_box_raw;
use crate::lexbor_abi::{self as lxb, LxbDoc, LxbNode};

pub type Parsed = lxb::parsed::Parsed;
type HtmlDoc = lxb::lxb_html_document_t;

const DOC_KIND_HTML: u32 = lxb::parsed::DOC_HTML;
const DOC_KIND_XML: u32 = lxb::parsed::DOC_XML;
const NODE_TYPE_DOCUMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT;
const LXB_STATUS_OK: u32 = lxb::lexbor_status_t_LXB_STATUS_OK;

pub use crate::dom_adapter::dom_index::dom_index_free;
pub use crate::dom_adapter::source_loc::lines_build;
pub use crate::dom_adapter::source_loc::lines_free;
pub use crate::dom_adapter::source_loc::pos_assign_to_dom;
pub use crate::dom_adapter::source_loc::pos_recorder_create;
pub use crate::dom_adapter::source_loc::pos_recorder_destroy;
pub use crate::dom_adapter::source_loc::pos_recorder_set_delegate;
pub use crate::dom_adapter::source_loc::pos_token_cb;
pub use crate::dom_adapter::text_index::text_index_free;
pub use crate::dom_adapter::utf8_input::utf8_sanitize;
use crate::dom_adapter::utf8_input::Sanitized;
pub use crate::xml::api::xml_doc_destroy;

extern "C" {

    fn lxb_html_document_destroy(doc: *mut HtmlDoc) -> *mut HtmlDoc;
    fn lxb_html_parse_chunk_begin(parser: *mut lxb::lxb_html_parser_t) -> *mut HtmlDoc;
    fn lxb_html_parse_chunk_process(
        parser: *mut lxb::lxb_html_parser_t,
        data: *const u8,
        size: usize,
    ) -> u32;
    fn lxb_html_parse_chunk_end(parser: *mut lxb::lxb_html_parser_t) -> u32;

}

/// The sanitiser's replacement buffer, freed however the parse exits.
struct CleanBuf {
    _owned: Option<OwnedBuf>,
}

/// The parser, destroyed however the parse exits. The parsed DOCUMENT is not
/// owned here - `parser_destroy` never frees it - which is exactly why it can be
/// returned.
struct Parser {
    p: *mut lxb::lxb_html_parser_t,
}

impl Drop for Parser {
    fn drop(&mut self) {
        if !self.p.is_null() {
            unsafe { lxb::lxb_html_parser_destroy(self.p) };
        }
    }
}

/// The position recorder, destroyed however the parse exits unless consumed.
struct RecorderHandle {
    r: *mut crate::dom_adapter::source_loc::Recorder,
}

impl Drop for RecorderHandle {
    fn drop(&mut self) {
        if !self.r.is_null() {
            unsafe { pos_recorder_destroy(self.r) };
        }
    }
}

/// Drive the low-level pipeline so element offsets can be captured, then build
/// the line table.
///
/// Returns the document, or NULL on failure with everything it allocated
/// released. `out_lines` receives the line table - possibly NULL if THAT
/// allocation failed, in which case line information degrades to nil rather than
/// failing the parse. That degradation is deliberate and is what
/// `spec/html_line_spec.rb`'s contract ("an Integer, or nil") allows.
unsafe fn parse_tracked(src: &[u8], out_lines: *mut *mut c_void) -> *mut HtmlDoc {
    *out_lines = core::ptr::null_mut();

    let parser = Parser {
        p: lxb::lxb_html_parser_create(),
    };
    if parser.p.is_null() || lxb::lxb_html_parser_init(parser.p) != LXB_STATUS_OK {
        return core::ptr::null_mut();
    }

    let doc = lxb_html_parse_chunk_begin(parser.p);
    if doc.is_null() {
        return core::ptr::null_mut();
    }

    /* Install the recorder, CHAINING the parser's own tree-building callback
     * (which chunk_begin has just set). If the recorder cannot be allocated we
     * simply parse without source tracking. */
    let mut rec = RecorderHandle {
        r: pos_recorder_create(src.as_ptr()),
    };
    if !rec.r.is_null() {
        let tkz = lxb::lxb_html_parser_tokenizer_noi(parser.p);
        /* Lexbor has a setter and a ctx getter for the token-done callback but
         * no getter for the callback FUNCTION, so that one field is read from
         * the struct directly; the ctx uses the public accessor. */
        pos_recorder_set_delegate(
            rec.r,
            (*tkz).callback_token_done,
            lxb::lxb_html_tokenizer_callback_token_done_ctx_noi(tkz),
        );
        lxb::lxb_html_tokenizer_callback_token_done_set_noi(
            tkz,
            Some(pos_token_cb),
            rec.r as *mut c_void,
        );
    }

    let mut st = lxb_html_parse_chunk_process(parser.p, src.as_ptr(), src.len());
    if st == LXB_STATUS_OK {
        st = lxb_html_parse_chunk_end(parser.p);
    }
    if st != LXB_STATUS_OK {
        lxb_html_document_destroy(doc);
        return core::ptr::null_mut();
    }

    if !rec.r.is_null() {
        pos_assign_to_dom(rec.r, doc as *mut LxbNode);
        /* Consumed: destroy it now rather than at the end of the scope, because
         * the line table below is the last thing that needs the input. */
        pos_recorder_destroy(rec.r);
        rec.r = core::ptr::null_mut();
        *out_lines = lines_build(src.as_ptr(), src.len());
    }

    doc
}

/// Parse `src` as an HTML document.
///
/// `assume_valid` skips the UTF-8 validation scan entirely - the caller has
/// already proved the bytes valid, typically from a Ruby String's cached
/// coderange. NULL on failure.
pub unsafe fn parse_html(src: *const u8, len: usize, assume_valid: bool) -> *mut Parsed {
    if src.is_null() && len != 0 {
        return core::ptr::null_mut();
    }

    let p = try_box_raw(Parsed {
        doc: core::ptr::null_mut(),
        kind: DOC_KIND_HTML,
        dom_index: core::ptr::null_mut(),
        newline_idx: core::ptr::null_mut(),
        text_index: core::ptr::null_mut(),
        evaluating: 0,
    });
    if p.is_null() {
        return core::ptr::null_mut();
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
            None => {
                drop(Box::from_raw(p));
                return core::ptr::null_mut(); /* OOM */
            }
        }
    }

    let bytes: &[u8] = if let Some(owned) = clean._owned.as_ref() {
        owned.as_slice()
    } else {
        if src.is_null() {
            &[]
        } else {
            core::slice::from_raw_parts(src, len)
        }
    };

    (*p).doc = parse_tracked(bytes, &mut (*p).newline_idx) as *mut c_void;
    drop(clean); /* the parse is done with the buffer, on every path */

    if (*p).doc.is_null() {
        /* parse_tracked left no line table when it failed, so nothing to free. */
        drop(Box::from_raw(p));
        return core::ptr::null_mut();
    }
    p
}

/// Free a parse handle and everything it owns.
pub unsafe fn parsed_destroy(p: *mut Parsed) {
    if p.is_null() {
        return;
    }

    /* The compat indices are HTML-only and built lazily; each free is a no-op
     * on NULL, so this is safe for an XML handle, which never sets them. */
    dom_index_free((*p).dom_index);
    (*p).dom_index = core::ptr::null_mut();
    lines_free((*p).newline_idx);
    (*p).newline_idx = core::ptr::null_mut();
    text_index_free((*p).text_index);
    (*p).text_index = core::ptr::null_mut();

    if !(*p).doc.is_null() {
        if (*p).kind == DOC_KIND_XML {
            xml_doc_destroy(Box::from_raw((*p).doc as *mut _)); /* whole-arena free */
        } else {
            lxb_html_document_destroy((*p).doc as *mut HtmlDoc);
        }
        (*p).doc = core::ptr::null_mut();
    }

    drop(Box::from_raw(p));
}

/* ---- document-kind accessors ---- */

pub unsafe fn parsed_kind(p: *const Parsed) -> u32 {
    (*p).kind
}

/// The HTML document. The C asserted the kind; here the assert is a debug one
/// for the same reason - a caller that gets this wrong has a bug the release
/// build cannot usefully recover from, and every caller checks `kind` first.
pub unsafe fn parsed_html_doc(p: *const Parsed) -> *mut HtmlDoc {
    debug_assert_eq!((*p).kind, DOC_KIND_HTML);
    (*p).doc as *mut HtmlDoc
}

/// Wrap an owned XML arena in a `kind = XML` handle. `xdoc` may be NULL
/// initially and set later, so a mid-parse failure still frees cleanly.
pub unsafe fn parsed_new_xml(xdoc: *mut c_void) -> *mut Parsed {
    try_box_raw(Parsed {
        doc: xdoc,
        kind: DOC_KIND_XML,
        dom_index: core::ptr::null_mut(),
        newline_idx: core::ptr::null_mut(),
        text_index: core::ptr::null_mut(),
        evaluating: 0,
    })
}

pub unsafe fn parsed_xml_doc(p: *const Parsed) -> *mut c_void {
    debug_assert_eq!((*p).kind, DOC_KIND_XML);
    (*p).doc
}

pub unsafe fn parsed_set_xml_doc(p: *mut Parsed, xdoc: *mut c_void) {
    debug_assert_eq!((*p).kind, DOC_KIND_XML);
    (*p).doc = xdoc;
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
