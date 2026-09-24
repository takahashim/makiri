//! HTML serialization primitives.
//!
//!   the node and its subtree -> the tree serializer
//!   the node's children only -> the deep serializer
//!
//! Lexbor streams the output in many small chunks (one per tag, attribute or
//! text piece). They are collected into a single growing C buffer, pre-reserved
//! to roughly the output size so those appends do not realloc on each geometric
//! step, and handed back as owned bytes; the Ruby-facing wrapper (into a String,
//! and the `Node#to_html` family) lives in [`crate::bridge::serialize`]. Lexbor
//! emits UTF-8.

#![allow(unsafe_code)]

use core::ffi::c_void;

use crate::cbuf::{buf_append, Buf};
use crate::lexbor::abi::consts::{
    STATUS_ERROR_MEMORY_ALLOCATION as LXB_STATUS_ERROR_MEMORY_ALLOCATION,
    STATUS_OK as LXB_STATUS_OK,
};
use crate::lexbor::abi::{
    lxb_html_serialize_deep_cb, lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF,
    lxb_html_serialize_pretty_deep_cb, lxb_html_serialize_pretty_tree_cb,
    lxb_html_serialize_tree_cb, LxbNode,
};
use crate::lexbor::adapter::arena_bytes::document_bytes;
use crate::lexbor::adapter::html::RawNode;

/// No pretty-printing option. The functions take the `int` typedef, the enum
/// is its own type, so the one conversion is spelled here.
const LXB_HTML_SERIALIZE_OPT_UNDEF: crate::lexbor::abi::lxb_html_serialize_opt_t =
    lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF as _;

/// What the serializer writes into, plus somewhere to put a panic.
struct SerCtx {
    buf: Buf,
    /// A panic in the append, latched: this is called from C, and unwinding
    /// into Lexbor aborts. The driver raises it after Lexbor has returned.
    panic: crate::caught::PanicLatch,
}

/// Lexbor's chunk sink. Must not panic INTO C: a panic is caught and reported
/// after the walk, the way an allocation failure already is.
unsafe extern "C" fn serialize_cb(data: *const u8, len: usize, ctx: *mut c_void) -> u32 {
    let c = &mut *(ctx as *mut SerCtx);
    let buf = &mut c.buf;
    c.panic.guard(LXB_STATUS_ERROR_MEMORY_ALLOCATION, || {
        if buf_append(buf, data as *const c_void, len).is_ok() {
            LXB_STATUS_OK
        } else {
            LXB_STATUS_ERROR_MEMORY_ALLOCATION
        }
    })
}

/// The buffer's ceiling and its initial reservation, both derived from the
/// document's live bytes in one walk.
///
/// The ceiling is the Lexbor analogue of the XML serializer's `arena_bytes` cap:
/// 32x the live bytes (covering escaping plus maximal pretty indentation) over a
/// 64 KiB floor - tight for a small document yet scaling with a large one, so a
/// legitimate parse round-trips through `to_html` (HTML parsing is itself
/// byte-uncapped) while a pathologically deep pretty-print fails closed instead
/// of growing without bound. It is deliberately *not* clamped to
/// `cbuf`'s hard ceiling here; `Buf` takes the minimum of the two on every
/// growth, so the clamp would only duplicate a constant this side could then
/// disagree with. The HTML tree cannot cycle (the mutation guards plus Lexbor's
/// own insert checks), so the ceiling is never reached in normal operation.
///
/// The reservation is ~live/4: serialized output is a fraction of the arena
/// (96-byte node structs dwarf their markup), which pre-sizes close to the real
/// output without a wasteful over-allocation and leaves geometric growth to
/// cover any underestimate.
fn serialize_sizes(live: usize) -> (usize, usize) {
    let cap = 65536usize.saturating_add(live.saturating_mul(32));
    let reserve = (live / 4).max(4096);
    (cap, reserve)
}

/// Serialize `node` into owned UTF-8 bytes. `deep` selects the children-only
/// (inner) serializer over the tree (outer) one; `pretty` selects indented
/// output. `None` is a Lexbor status failure (the buffer is freed).
pub fn serialize(node: RawNode, deep: bool, pretty: bool) -> Option<Buf> {
    // SAFETY: `node` came from a live wrapper, so it and its document are live.
    let doc = unsafe { node.as_node() }.owner_document();
    let (cap, reserve) = serialize_sizes(document_bytes(doc));
    let node = node.as_ptr() as *mut LxbNode;

    let mut c = SerCtx {
        buf: Buf::new(cap),
        panic: crate::caught::PanicLatch::new(),
    };
    // SAFETY: the buffer is freed by `Buf`'s Drop however this exits, including
    // the panic the latch re-raises below.
    unsafe {
        let _ = c.buf.reserve(reserve); /* best-effort pre-size */

        let ctx = &mut c as *mut SerCtx as *mut c_void;
        let st = match (deep, pretty) {
            (true, true) => lxb_html_serialize_pretty_deep_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                Some(serialize_cb),
                ctx,
            ),
            (true, false) => lxb_html_serialize_deep_cb(node, Some(serialize_cb), ctx),
            (false, true) => lxb_html_serialize_pretty_tree_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                Some(serialize_cb),
                ctx,
            ),
            (false, false) => lxb_html_serialize_tree_cb(node, Some(serialize_cb), ctx),
        };

        /* Lexbor has returned, so this is the first frame where a panic the
         * sink caught can be raised. `Buf`'s Drop frees what was written. */
        c.panic.resume();

        if st != LXB_STATUS_OK {
            return None; /* `Buf`'s Drop frees it */
        }
        Some(c.buf)
    }
}
