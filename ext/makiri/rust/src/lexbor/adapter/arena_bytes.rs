//! How much memory a Lexbor document's arena holds: the bytes in use, which a
//! serializer sizes its buffer from, and the bytes allocated, which the GC is
//! told about.
//!
//! Lexbor keeps no running total, so both walk the chunk lists of the
//! document's node and text pools.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::lexbor::abi as lxb;

use super::html::HtmlDoc;

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

/// Sum the node and text pools of `doc`.
fn document_pools(doc: HtmlDoc<'_>, measure: Measure) -> usize {
    let doc = doc.as_raw();
    let mut total = 0usize;
    // SAFETY: a live document's two pools, whose chunk lists are only read.
    for pool in unsafe { [(*doc).mraw, (*doc).text] } {
        if pool.is_null() {
            continue;
        }
        // SAFETY: as above.
        let bytes = unsafe { mem_total((*pool).mem, measure) };
        total = match total.checked_add(bytes) {
            Some(t) => t,
            None => return usize::MAX,
        };
    }
    total
}

/// The live bytes in `doc`'s arena, which the serializers size their buffer
/// from.
pub fn document_bytes(doc: HtmlDoc<'_>) -> usize {
    document_pools(doc, Measure::Used)
}

/// The bytes `doc`'s arena has allocated, used or not - what it costs the
/// process, for `HtmlParsed::external_bytes`.
pub fn document_capacity(doc: HtmlDoc<'_>) -> usize {
    document_pools(doc, Measure::Capacity)
}
