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
use crate::lexbor::adapter::arena_bytes::{document_size, DocumentSize};
use crate::lexbor::adapter::html::{HtmlDoc, HtmlNode, RawNode};
use crate::lexbor::chunks::{chunk_cb, ChunkSink, Chunks};
use crate::node_type::NodeType;

/// Every document's ceiling is at least this, so a serialization can start
/// under it before the document has been measured.
const CEILING_FLOOR: usize = 65536;

/// The buffer's ceiling for a document of `size`.
///
/// The Lexbor analogue of the XML serializer's `arena_bytes` cap: 32x the live
/// bytes (covering escaping plus maximal pretty indentation), plus every node
/// writing the longest name the document interns four times (a start and an
/// end tag, each up to a prefix and a local name that long), over
/// [`CEILING_FLOOR`]. The name term is what `live` cannot see: a name is
/// stored once however many nodes carry it, and 50 custom elements named by
/// 20,000 bytes failed their own `to_html` without it; for ordinary names it
/// is a few percent of the first term. Tight for a small document yet scaling
/// with a large one, so a legitimate parse round-trips through `to_html` (HTML
/// parsing is itself byte-uncapped) while a pathologically deep pretty-print
/// fails closed instead of growing without bound. It is deliberately *not*
/// clamped to `cbuf`'s hard ceiling here; `Buf` takes the minimum of the two
/// on every growth, so the clamp would only duplicate a constant this side
/// could then disagree with. The HTML tree cannot cycle (the mutation guards
/// plus Lexbor's own insert checks), so the ceiling is never reached in normal
/// operation.
fn ceiling(size: DocumentSize) -> usize {
    let names = size
        .nodes
        .saturating_mul(size.longest_name.saturating_mul(4));
    CEILING_FLOOR
        .saturating_add(size.live.saturating_mul(32))
        .saturating_add(names)
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
        self.buf.raise_limit(ceiling(document_size(doc)));
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
    let size = document_size(doc);
    let mut buf = Buf::new(ceiling(size));
    let _ = buf.reserve((size.live / 4).max(4096)); /* best-effort pre-size */
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
    let handle = unsafe { node.as_node() };
    let mut c = Chunks::new(sink_for(handle));

    // SAFETY: a live node (above); the sink accepts every chunk with its own
    // context, and nothing changes the tree while Ruby is not running.
    unsafe {
        let ctx = c.ctx();
        /* The walks are ours (`HtmlNode::serialize_to`), Lexbor writing each
         * node: see `adapter::html::serialize` for what they decide. */
        let st = if pretty {
            handle.serialize_pretty_to(deep, Some(chunk_cb::<Sink>), ctx)
        } else {
            handle.serialize_to(deep, Some(chunk_cb::<Sink>), ctx)
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
