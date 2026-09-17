//! HTML serialization primitives (glue/ruby_html_serialize.c).
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
use crate::lexbor::adapter::html::RawNode;
use crate::lexbor::adapter::post_parse::lxb_document_bytes;
use crate::lexbor::ffi::{
    LxbNode, LXB_HTML_SERIALIZE_OPT_UNDEF, LXB_STATUS_ERROR_MEMORY_ALLOCATION, LXB_STATUS_OK,
};

/// Lexbor's chunk sink. Must not panic: it is called from C.
unsafe extern "C" fn serialize_cb(data: *const u8, len: usize, ctx: *mut c_void) -> u32 {
    if buf_append(ctx as *mut Buf, data as *const c_void, len) == crate::cbuf::BUF_OK {
        LXB_STATUS_OK
    } else {
        LXB_STATUS_ERROR_MEMORY_ALLOCATION
    }
}

extern "C" {
    fn lxb_html_serialize_tree_cb(
        node: *mut LxbNode,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_html_serialize_deep_cb(
        node: *mut LxbNode,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_html_serialize_pretty_tree_cb(
        node: *mut LxbNode,
        opt: u32,
        indent: usize,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
    fn lxb_html_serialize_pretty_deep_cb(
        node: *mut LxbNode,
        opt: u32,
        indent: usize,
        cb: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> u32,
        ctx: *mut c_void,
    ) -> u32;
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
/// `MKR_BUF_HARD_MAX` here; `mkr_buf.c` takes the minimum of the two on every
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
    let node = node.as_ptr() as *mut LxbNode;
    // SAFETY: `node` came from a live wrapper, so its document is live too.
    let (cap, reserve) = serialize_sizes(unsafe { lxb_document_bytes(node) });

    let mut buf = Buf::new(cap);
    // SAFETY: `buf` is freed on both paths below, and nothing between here and
    // there can raise - the Err is returned, not thrown.
    unsafe {
        let _ = buf.reserve(reserve); /* best-effort pre-size */

        let ctx = &mut buf as *mut Buf as *mut c_void;
        let st = match (deep, pretty) {
            (true, true) => lxb_html_serialize_pretty_deep_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                serialize_cb,
                ctx,
            ),
            (true, false) => lxb_html_serialize_deep_cb(node, serialize_cb, ctx),
            (false, true) => lxb_html_serialize_pretty_tree_cb(
                node,
                LXB_HTML_SERIALIZE_OPT_UNDEF,
                0,
                serialize_cb,
                ctx,
            ),
            (false, false) => lxb_html_serialize_tree_cb(node, serialize_cb, ctx),
        };

        if st != LXB_STATUS_OK {
            buf.free();
            return None;
        }
        Some(buf)
    }
}
