//! Per-document text-extraction index.
//!
//! Descendant-text aggregation (`Node#text`, the XPath string-value) otherwise
//! walks every node of a subtree chasing pointers through Lexbor's 96-byte
//! nodes; at scale that is cache-bound and dominates the cost. This index
//! removes the per-extraction walk: one build pass records, in document order, a
//! flat array of every TEXT/CDATA node's byte slice, plus a pointer-keyed map
//! from each element or fragment to the contiguous `[start, end)` run of slices
//! its subtree owns. A later `Node#text` is a hash lookup and a single pre-sized
//! memcpy run over a cache-dense slice array, touching no element node at all.
//!
//! # The slices are stored raw on purpose
//!
//! A stored `&[u8]` would be a stronger claim than the truth. The nodes
//! belong to Lexbor, and its mutation API writes through them outside Rust's borrow
//! checker, so a long-lived Rust reference would assert a lifetime nothing
//! enforces. What actually keeps a cached slice valid is a RUNTIME protocol:
//! every mutation goes through one invalidation hook
//! (`HtmlParsed::invalidate_indexes`), which drops the whole index, so a
//! slice can never outlive the storage it points into. The arena also never
//! frees a node - detached, never destroyed - so only a mutation can reallocate
//! the text a slice borrows.
//!
//! So the index keeps a private [`RawSlice`] and hands out ordinary `&[u8]`
//! borrowed from `&self` ([`TextIndex::slices_of`] / [`TextRun`]): holding the
//! borrow holds the index, and dropping the index is the only thing a mutation
//! does to it. The one `unsafe` that turns a stored slice back into bytes is
//! [`RawSlice::get`], and that protocol is its safety argument.
//!
//! # Fail-closed
//!
//! A build that cannot allocate leaves the index unbuilt and answers 0, and the
//! caller falls back to its own walk. It must never answer a SHORTER run: a
//! truncated text is a well-formed wrong answer, which is exactly what the
//! project's fail-closed rule is about.

#![allow(unsafe_code)]

use crate::falloc::{try_vec_with_capacity, VecPush};
use crate::ptr_table::PtrTable;
use core::ptr::NonNull;

use super::html::{HtmlNode, NodeType, RawNode};

/// One container's slice run. `start`/`end` are INDICES into `slices`, not byte
/// offsets, and are `u32` because the build refuses to index a document with
/// more than `u32::MAX` slices (see [`TextIndex::build`]).
#[derive(Clone, Copy)]
struct Run {
    start: u32,
    end: u32,
}

/// A TEXT/CDATA node's character data, as the address and length of the
/// arena storage it borrows. Private, and only ever stored inside a
/// [`TextIndex`], so borrowing one borrows the index. Neither `Clone` nor
/// `Copy`, so it cannot be taken out of the index.
struct RawSlice(NonNull<[u8]>);

impl RawSlice {
    /// The bytes, for as long as the slice - and so the index holding it - is
    /// borrowed.
    fn get(&self) -> &[u8] {
        // SAFETY: the pointer is a character-data node's own storage in the
        // document's arena, taken by `TextIndex::build`. The arena never frees
        // a node (detached, never destroyed), and the only thing that can move
        // or rewrite the text is a mutation - every one of which goes through
        // `HtmlParsed::invalidate_indexes`, which DROPS the index first. The
        // result borrows `self`, which lives inside the index, so it cannot
        // outlive that drop.
        unsafe { self.0.as_ref() }
    }
}

/// The run of text slices one container's subtree owns, borrowed from the
/// index: the bytes in document order, and their total length.
#[derive(Clone, Copy)]
pub struct TextRun<'i> {
    slices: &'i [RawSlice],
    total: usize,
}

impl<'i> TextRun<'i> {
    /// The slices, in document order. Never empty ones.
    pub fn iter(self) -> impl Iterator<Item = &'i [u8]> {
        self.slices.iter().map(RawSlice::get)
    }

    /// The byte total of the run: the sum of the slices' lengths.
    pub fn total(self) -> usize {
        self.total
    }
}

pub struct TextIndex {
    /// Document-order TEXT/CDATA slices, borrowed from the arena.
    slices: Vec<RawSlice>,
    /// `slices.len() + 1` entries; `prefix[i]` is the byte count before slice
    /// `i`. Always non-empty, so an empty `[0, 0)` run over a text-free subtree
    /// reads `prefix[0]` rather than nothing.
    prefix: Vec<usize>,
    /// Container -> slice run.
    runs: PtrTable<Option<RawNode>, Run>,
}

#[inline]
fn is_container(n: HtmlNode<'_>) -> bool {
    matches!(
        n.node_type(),
        NodeType::Element | NodeType::DocumentFragment
    )
}

/// The non-empty character data of a TEXT/CDATA node, or `None`.
///
/// The single "does this node contribute a slice" test, shared by the counting
/// and filling passes so the two-pass sizing and the fill can never disagree
/// about which nodes yield one - a disagreement would size the array for a
/// different set than it fills.
#[inline]
fn text_slice(n: HtmlNode<'_>) -> Option<&[u8]> {
    n.char_data().filter(|d| !d.is_empty())
}

/// Pass 1: count the text slices and the containers under `root` (inclusive),
/// so each array is sized exactly once.
fn count(root: HtmlNode<'_>) -> (usize, usize) {
    let (mut slices, mut containers) = (0usize, 0usize);
    for n in root.subtree() {
        if is_container(n) {
            containers += 1;
        } else if text_slice(n).is_some() {
            slices += 1;
        }
    }
    (slices, containers)
}

/// An explicit DFS frame. Recursion is avoided so a deep tree cannot exhaust the
/// stack - the same discipline as the attr/element index.
struct Frame<'d> {
    /// The next child to visit.
    child: Option<HtmlNode<'d>>,
    /// This open container's slot in `runs`.
    slot: usize,
}

impl TextIndex {
    /// Build over `root` (the document root element). `None` when `root` is not
    /// a container - `lxb_dom_document_root` answers with the first child when
    /// the document has no `<html>`, and that can be a leaf - or on OOM; both
    /// are fail-closed and the caller walks instead.
    pub(crate) fn build(root: HtmlNode<'_>) -> Option<TextIndex> {
        /* The index is ROOTED at a container: pass 2 opens `root`'s own run
         * before it looks at anything, and the run table is sized from the
         * container count, which does not count `root` when it is a leaf. The
         * caller can hand us one: `lxb_dom_document_root` answers with the
         * document's first child when the document has no `<html>`, and a
         * script can put a comment or a processing instruction there. Walk
         * instead. */
        if !is_container(root) {
            return None;
        }

        let (nslices, ncont) = count(root);

        /* Run bounds are u32 in the run table. A document with more than
         * u32::MAX text slices is impossible in practice (each is >= 1 byte),
         * but guard it anyway rather than truncate the index. */
        if nslices > u32::MAX as usize {
            return None;
        }

        /* Both arrays are sized EXACTLY here, so every push below lands in
         * reserved capacity and cannot allocate - which is why they use a plain
         * `push`. See clippy.toml on why `push` after a successful reserve is
         * deliberately not banned. */
        let empty = Run { start: 0, end: 0 };
        let mut t = TextIndex {
            slices: try_vec_with_capacity(nslices)?,
            prefix: try_vec_with_capacity(nslices.checked_add(1)?)?,
            runs: PtrTable::with_keys(ncont, empty)?,
        };
        t.prefix.push(0);

        /* Pass 2: explicit DFS recording each container's slice run. The frame
         * stack is the one array whose size is not known in advance (it is
         * bounded by tree DEPTH, not node count), so it grows through falloc.
         * The run table was sized for exactly the containers pass 1 counted, so
         * a refused insert means the tree changed under us: fail closed. */
        let mut stack: Vec<Frame<'_>> = try_vec_with_capacity(1)?;
        let slot = t.runs.insert(Some(RawNode::from(root)), empty)?;
        stack.push(Frame {
            child: root.first_child(),
            slot,
        });

        while let Some(top) = stack.last_mut() {
            let Some(child) = top.child else {
                /* Close this subtree. */
                let slot = top.slot;
                t.runs.slot_mut(slot).end = t.slices.len() as u32;
                stack.pop();
                continue;
            };
            top.child = child.next(); /* advance the cursor for our return */

            if let Some(text) = text_slice(child) {
                /* Compared against the COUNT, not against `capacity()`: a Vec
                 * may reserve more than asked, so capacity would not catch a
                 * count/fill disagreement. One more slice than pass 1 saw means
                 * the tree changed under us - fail closed rather than push past
                 * the reservation. */
                if t.slices.len() == nslices {
                    return None;
                }
                /* The running total is checked rather than wrapped: a wrapped
                 * prefix would make a later `prefix[end] - prefix[start]`
                 * smaller than the bytes actually present, which is a short read
                 * into a pre-sized String. */
                let total = t.prefix[t.slices.len()].checked_add(text.len())?;
                /* Stored raw: see the module docs, and `RawSlice::get`. */
                t.slices.push(RawSlice(NonNull::from(text)));
                t.prefix.push(total);
            } else if is_container(child) {
                let start = t.slices.len() as u32;
                let slot = t
                    .runs
                    .insert(Some(RawNode::from(child)), Run { start, end: start })?;
                stack
                    .falloc_push(Frame {
                        child: child.first_child(),
                        slot,
                    })
                    .ok()?;
            }
            /* Other kinds (comment / PI / doctype) are childless leaves. */
        }

        Some(t)
    }
}

/* ------------------------------------------------------------------ *
 * public surface                                                     *
 * ------------------------------------------------------------------ */

impl TextIndex {
    /// The document-order run of text slices `node`'s subtree owns, with its
    /// byte total; None for a node outside the indexed tree. Never a shorter
    /// run than the truth.
    pub fn slices_of(&self, node: RawNode) -> Option<TextRun<'_>> {
        let r = self.runs.get(Some(node))?;
        let (start, end) = (r.start as usize, r.end as usize);
        Some(TextRun {
            slices: &self.slices[start..end],
            total: self.prefix[end] - self.prefix[start],
        })
    }
}
