//! How much memory a Lexbor document's arena holds: the bytes in use, which a
//! serializer sizes its buffer from, and the bytes allocated, which the GC is
//! told about.
//!
//! Lexbor keeps no running total, so both walk the chunk lists of the
//! document's node and text pools.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::lexbor_abi::{self as lxb, LxbDoc, LxbNode};

use super::html::TYPE_DOCUMENT as NODE_TYPE_DOCUMENT;

/// Which of a chunk's two sizes to sum.
#[derive(Clone, Copy)]
enum Measure {
    /// Bytes handed out (`length`): what a serializer will write.
    Used,
    /// Bytes allocated (`size`): what the process paid for.
    Capacity,
}

/// Bytes of one Lexbor mem pool.
///
/// Lexbor exposes no running total, so the chunk list is walked summing each
/// chunk. Cheap: the chunks are few and large. Saturates to `usize::MAX` on the
/// unreachable overflow; the callers clamp anyway.
unsafe fn mem_total(mem: *const lxb::lexbor_mem_t, measure: Measure) -> usize {
    let mut total = 0usize;
    let mut c = if mem.is_null() {
        core::ptr::null_mut()
    } else {
        (*mem).chunk_first
    };
    while !c.is_null() {
        let n = match measure {
            Measure::Used => (*c).length,
            Measure::Capacity => (*c).size,
        };
        total = match total.checked_add(n) {
            Some(t) => t,
            None => return usize::MAX,
        };
        c = (*c).next;
    }
    total
}

/// Sum the node and text pools of a node's document.
unsafe fn document_pools(node: *mut LxbNode, measure: Measure) -> usize {
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
        total = match total.checked_add(mem_total((*pool).mem, measure)) {
            Some(t) => t,
            None => return usize::MAX,
        };
    }
    total
}

/// The live bytes in a node's document arena, which the serializers size their
/// buffer from.
pub unsafe fn document_bytes(node: *mut LxbNode) -> usize {
    document_pools(node, Measure::Used)
}

/// The bytes a node's document arena has allocated, used or not - what it
/// costs the process, for `HtmlParsed::external_bytes`.
pub unsafe fn document_capacity(node: *mut LxbNode) -> usize {
    document_pools(node, Measure::Capacity)
}
