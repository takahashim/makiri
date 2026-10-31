//! How much memory a Lexbor document's arena holds: the bytes in use, which a
//! serializer sizes its buffer from, and the bytes allocated, which the GC is
//! told about.
//!
//! Lexbor keeps no running total, so both walk the chunk lists of what the
//! document owns: its node and text pools AND its four name tables - tags,
//! attribute names, namespaces and prefixes (`lexbor_hash_t`), each an entry
//! pool plus a pool for names too long to store inline. The tables are not a
//! rounding error: a custom element's tag name, however long, is interned
//! there and nowhere else, so a document of 50 elements named by 20,000 bytes
//! measured ~2 KB of pools and failed its own `to_html`.
//!
//! A document made with an OWNER (`lxb_dom_document_interface_init`'s owner
//! case, `node.owner_document != self`) shares all six with that owner and
//! frees none of them. It is measured as its owner's arena for the ceiling -
//! that is what its nodes live in - and as nothing for the GC, which is told
//! about the owner once, by the owner.

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

/// Bytes of one Lexbor name table: its entry pool, and the pool holding the
/// names too long for an entry; for [`Measure::Capacity`] also the bucket
/// array, which is a malloc of its own.
///
/// # Safety
/// `hash` must be null or a live, initialised table.
unsafe fn hash_total(hash: *const lxb::lexbor_hash_t, measure: Measure) -> usize {
    if hash.is_null() {
        return 0;
    }
    let entries = (*hash).entries;
    let names = (*hash).mraw;
    let mut total = 0usize;
    if !entries.is_null() {
        total = total.saturating_add(mem_total((*entries).mem, measure));
    }
    if !names.is_null() {
        total = total.saturating_add(mem_total((*names).mem, measure));
    }
    if matches!(measure, Measure::Capacity) && !(*hash).table.is_null() {
        let table = (*hash)
            .table_size
            .saturating_mul(core::mem::size_of::<*mut lxb::lexbor_hash_entry_t>());
        total = total.saturating_add(table);
    }
    total
}

/// What one document owns, by part, in one [`Measure`].
#[derive(Clone, Copy, Default)]
struct Owned {
    /// The node pool (`mraw`).
    nodes: usize,
    /// The text pool.
    text: usize,
    /// The four name tables together.
    tables: usize,
}

impl Owned {
    fn total(self) -> usize {
        self.nodes
            .saturating_add(self.text)
            .saturating_add(self.tables)
    }
}

/// Measure everything `doc` owns - its two pools and its four name tables -
/// or nothing when it owns none of them (it was made with an owner; see the
/// module doc). Each chunk list is walked ONCE: a walk is a cache miss per
/// chunk, thousands of them for a large document, and a second walk of the
/// node pool measured as a 7% slower `to_html` on a 5 MB page.
fn owned_arena(doc: HtmlDoc<'_>, measure: Measure) -> Owned {
    let doc = doc.as_raw();
    // SAFETY: a live document; only its own fields are read.
    if unsafe { (*doc).node.owner_document } != doc {
        return Owned::default();
    }
    // SAFETY: a live document that owns these pools, whose chunk lists are
    // only read.
    let pool = |p: *mut lxb::lexbor_mraw_t| unsafe {
        if p.is_null() {
            0
        } else {
            mem_total((*p).mem, measure)
        }
    };
    let mut tables = 0usize;
    // SAFETY: as above; each table is null or initialised by the document.
    for hash in unsafe { [(*doc).tags, (*doc).attrs, (*doc).ns, (*doc).prefix] } {
        // SAFETY: as above.
        tables = tables.saturating_add(unsafe { hash_total(hash, measure) });
    }
    // SAFETY: as above.
    let (nodes, text) = unsafe { (pool((*doc).mraw), pool((*doc).text)) };
    Owned {
        nodes,
        text,
        tables,
    }
}

/// What a serializer needs to know about the document it writes: enough to
/// bound the output of a LEGITIMATE document from above.
#[derive(Clone, Copy, Debug)]
pub struct DocumentSize {
    /// The live bytes of the arena `doc`'s nodes live in - its own, or its
    /// owner's: pools and name tables together. Text, attribute values and
    /// every distinct name are in here once.
    pub live: usize,
    /// At most this many nodes: the node pool's live bytes over the smallest
    /// node struct. An over-count - the pool holds more than nodes - which
    /// is the safe direction for a bound.
    pub nodes: usize,
    /// The longest interned name - tag, attribute, prefix or namespace URI.
    /// A name is stored ONCE and written once per node that carries it, so
    /// `live` alone says nothing about how often it is written: 50 elements
    /// named by 20,000 bytes are ~20 KB of arena and 2 MB of markup.
    pub longest_name: usize,
}

/// Measure the arena `doc`'s nodes live in; see [`DocumentSize`]. Walks the
/// chunk lists and the name tables' buckets: time in the number of chunks
/// and distinct names, not in the document's size.
pub fn document_size(doc: HtmlDoc<'_>) -> DocumentSize {
    let owner = arena_owner(doc);
    let raw = owner.as_raw();
    let arena = owned_arena(owner, Measure::Used);
    let mut longest_name = 0usize;
    // SAFETY: a live document's tables, null or initialised; only read.
    for hash in unsafe { [(*raw).tags, (*raw).attrs, (*raw).ns, (*raw).prefix] } {
        // SAFETY: as above.
        longest_name = longest_name.max(unsafe { longest_entry(hash) });
    }
    DocumentSize {
        live: arena.total(),
        nodes: arena.nodes / core::mem::size_of::<lxb::lxb_dom_node_t>(),
        longest_name,
    }
}

/// The longest key in a Lexbor name table, from its buckets' chains.
///
/// # Safety
/// `hash` must be null or a live, initialised table.
unsafe fn longest_entry(hash: *const lxb::lexbor_hash_t) -> usize {
    if hash.is_null() || (*hash).table.is_null() {
        return 0;
    }
    let table = (*hash).table;
    let mut longest = 0usize;
    for i in 0..(*hash).table_size {
        // SAFETY: the table holds `table_size` bucket heads, each null or the
        // first of a chain of live entries.
        let mut e = *table.add(i);
        while !e.is_null() {
            longest = longest.max((*e).length);
            e = (*e).next;
        }
    }
    longest
}

/// The bytes `doc` has allocated, used or not - what it costs the process, for
/// `HtmlParsed::external_bytes`. 0 for a document that owns no arena of its
/// own, so a shared one is never reported twice.
pub fn document_capacity(doc: HtmlDoc<'_>) -> usize {
    owned_arena(doc, Measure::Capacity).total()
}

/// How many chunks `doc`'s node and text pools hold - 0 when it owns none
/// (made with an owner). O(1): Lexbor counts them (`chunk_length`), so this
/// is the cheap signal that a document has grown, where
/// [`document_capacity`] walks every chunk to say by how much.
pub fn document_chunks(doc: HtmlDoc<'_>) -> usize {
    let doc = doc.as_raw();
    // SAFETY: a live document; only its own fields are read.
    if unsafe { (*doc).node.owner_document } != doc {
        return 0;
    }
    // SAFETY: a live document that owns these pools; each is null or
    // initialised, with its `mem` set by that initialisation.
    let pool = |p: *mut lxb::lexbor_mraw_t| unsafe {
        if p.is_null() || (*p).mem.is_null() {
            0
        } else {
            (*(*p).mem).chunk_length
        }
    };
    // SAFETY: as above.
    unsafe { pool((*doc).mraw).saturating_add(pool((*doc).text)) }
}

/// The document whose arena `doc` allocates from: `doc` itself, or the owner
/// it was made with.
fn arena_owner(doc: HtmlDoc<'_>) -> HtmlDoc<'_> {
    // SAFETY: a live document's `owner_document` is itself or the owner whose
    // arena it allocates from, which outlives it; null (never set) falls back.
    unsafe { HtmlDoc::from_raw((*doc.as_raw()).node.owner_document) }.unwrap_or(doc)
}
