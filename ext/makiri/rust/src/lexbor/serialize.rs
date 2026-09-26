//! HTML serialization primitives.
//!
//!   the node and its subtree -> the tree serializer
//!   the node's children only -> the deep serializer
//!
//! Lexbor streams the output in many small chunks (one per tag, attribute or
//! text piece). They are collected into a single growing C buffer - for the
//! whole document pre-reserved to roughly the output size, so those appends do
//! not realloc on each geometric step - and handed back as owned bytes; the
//! Ruby-facing wrapper (into a String, and the `Node#to_html` family) lives in
//! [`crate::bridge::serialize`]. Lexbor emits UTF-8.

#![allow(unsafe_code)]

use crate::cbuf::{Buf, BufError};
use crate::lexbor::abi::consts::STATUS_OK as LXB_STATUS_OK;
use crate::lexbor::abi::{
    lxb_html_serialize_deep_cb, lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF,
    lxb_html_serialize_pretty_deep_cb, lxb_html_serialize_pretty_tree_cb,
    lxb_html_serialize_tree_cb,
};
use crate::lexbor::adapter::arena_bytes::document_bytes;
use crate::lexbor::adapter::html::{HtmlDoc, HtmlNode, RawNode};
use crate::lexbor::chunks::{chunk_cb, ChunkSink, Chunks};
use crate::node_type::NodeType;

/// No pretty-printing option. The functions take the `int` typedef, the enum
/// is its own type, so the one conversion is spelled here.
const LXB_HTML_SERIALIZE_OPT_UNDEF: crate::lexbor::abi::lxb_html_serialize_opt_t =
    lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF as _;

/// Every document's ceiling is at least this, so a serialization can start
/// under it before the document has been measured.
const CEILING_FLOOR: usize = 65536;

/// The buffer's ceiling for a document of `live` arena bytes.
///
/// The Lexbor analogue of the XML serializer's `arena_bytes` cap: 32x the live
/// bytes (covering escaping plus maximal pretty indentation) over
/// [`CEILING_FLOOR`] - tight for a small document yet scaling with a large one,
/// so a legitimate parse round-trips through `to_html` (HTML parsing is itself
/// byte-uncapped) while a pathologically deep pretty-print fails closed instead
/// of growing without bound. It is deliberately *not* clamped to `cbuf`'s hard
/// ceiling here; `Buf` takes the minimum of the two on every growth, so the
/// clamp would only duplicate a constant this side could then disagree with.
/// The HTML tree cannot cycle (the mutation guards plus Lexbor's own insert
/// checks), so the ceiling is never reached in normal operation.
fn ceiling(live: usize) -> usize {
    CEILING_FLOOR.saturating_add(live.saturating_mul(32))
}

/// The output buffer, plus the document whose ceiling it has not taken yet.
///
/// Measuring a document walks its arena's chunk lists, which costs time in the
/// document's size. A subtree - whose output may be a few bytes of a large
/// document - therefore starts under [`CEILING_FLOOR`] and measures only when
/// its output passes that: the ceiling it ends up under is the same one.
struct Sink<'d> {
    buf: Buf,
    unmeasured: Option<HtmlDoc<'d>>,
}

impl Sink<'_> {
    /// The chunk would pass the floor: take the document's real ceiling and
    /// try again, once.
    #[cold]
    #[inline(never)]
    fn measure_and_retry(&mut self, bytes: &[u8]) -> bool {
        let Some(doc) = self.unmeasured.take() else {
            return false;
        };
        self.buf.raise_limit(ceiling(document_bytes(doc)));
        self.buf.append(bytes).is_ok()
    }
}

impl ChunkSink for Sink<'_> {
    #[inline]
    fn take(&mut self, bytes: &[u8]) -> bool {
        match self.buf.append(bytes) {
            Ok(()) => true,
            Err(BufError::Limit) => self.measure_and_retry(bytes),
            Err(BufError::Oom) => false,
        }
    }
}

/// The sink to serialize `node` into.
///
/// The document, and its root element, are measured up front and the buffer
/// pre-reserved to ~live/4: serialized output is a fraction of the arena
/// (96-byte node structs dwarf their markup), which pre-sizes close to the real
/// output without a wasteful over-allocation and spares the whole-document path
/// the per-step reallocs, where they measured. Any other node is a subtree the
/// arena's size says nothing about, so its buffer grows geometrically from
/// empty, and the document is measured only if the output passes the floor.
fn sink_for(node: HtmlNode<'_>) -> Sink<'_> {
    let doc = node.owner_document();
    let whole =
        node.node_type() == NodeType::Document || doc.as_node().document_root() == Some(node);
    if !whole {
        return Sink {
            buf: Buf::new(CEILING_FLOOR),
            unmeasured: Some(doc),
        };
    }
    let live = document_bytes(doc);
    let mut buf = Buf::new(ceiling(live));
    let _ = buf.reserve((live / 4).max(4096)); /* best-effort pre-size */
    Sink {
        buf,
        unmeasured: None,
    }
}

/// Serialize `node` into owned UTF-8 bytes. `deep` selects the children-only
/// (inner) serializer over the tree (outer) one; `pretty` selects indented
/// output. `None` is a Lexbor status failure (the buffer is freed).
pub fn serialize(node: RawNode, deep: bool, pretty: bool) -> Option<Buf> {
    // SAFETY: `node` came from a live wrapper, so it and its document are live.
    let mut c = Chunks::new(sink_for(unsafe { node.as_node() }));
    let node = node.as_lxb_mut();

    // SAFETY: the buffer is freed by `Buf`'s Drop however this exits, including
    // the panic the latch re-raises below.
    unsafe {
        let ctx = c.ctx();
        let st = match (deep, pretty) {
            (true, true) => lxb_html_serialize_pretty_deep_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                Some(chunk_cb::<Sink>),
                ctx,
            ),
            (true, false) => lxb_html_serialize_deep_cb(node, Some(chunk_cb::<Sink>), ctx),
            (false, true) => lxb_html_serialize_pretty_tree_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                Some(chunk_cb::<Sink>),
                ctx,
            ),
            (false, false) => lxb_html_serialize_tree_cb(node, Some(chunk_cb::<Sink>), ctx),
        };

        /* Lexbor has returned, so this is the first frame where a panic the
         * sink caught can be raised. `Buf`'s Drop frees what was written. */
        c.panic.resume();

        /* Lexbor stops on the refusing chunk's status; `refused` is checked
         * as well, so a sink that said no can never pass for a whole output. */
        if st != LXB_STATUS_OK || c.refused {
            return None; /* `Buf`'s Drop frees it */
        }
        Some(c.sink.buf)
    }
}
