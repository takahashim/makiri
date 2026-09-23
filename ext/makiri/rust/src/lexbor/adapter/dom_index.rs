//! The per-document element index: **tag id -> elements**, a CSR layout - a
//! flat `tag_nodes` array of every element grouped by tag id and kept in
//! document order, with `tag_off` prefix-sum offsets, so bucket `t` is
//! `tag_nodes[tag_off[t]..tag_off[t+1]]`. This is what lets the XPath engine
//! answer `//tag` without walking.
//!
//! Two tree passes: count and size, then fill. The arrays are therefore sized
//! once and never grow mid-build, which is what keeps the OOM path trivial -
//! there is nothing half-built to unwind.
//!
//! It used to carry an attribute -> owner table too, and to backfill each
//! attribute's `node.parent` from it, on the premise that Lexbor links neither.
//! Lexbor does keep `attr->owner` (set on append, cleared on removal), and
//! `HtmlNode::parent` now reads it live - the backfill answered whatever held
//! when the index was last built, so a detached element's attribute had a
//! parent or not depending on history. The index no longer writes to the tree.
//!
//! # Fail-closed
//!
//! An allocation failure leaves the index UNBUILT and returns "no index", so the
//! caller either walks or raises. It must not leave a partially filled one: a
//! lookup that misses reads as "this attribute has no owner", which is a
//! well-formed wrong answer.

#![forbid(unsafe_code)]

use crate::falloc::try_vec_with_capacity;
use crate::lexbor::abi::LxbNode;

/// Tag buckets cover only Lexbor's STATIC tag-id range `[1, LXB_TAG__LAST_ENTRY)`,
/// so the end of that range doubles as this index's capacity.
///
/// A custom element's tag id is the pointer to its interned tag data
/// (`lxb_tag_append` sets `data->tag_id = (lxb_tag_id_t) data`), an enormous
/// value that cannot key a dense array. Those elements are simply left out, and
/// `//customtag` falls back to a tree walk - rare in practice.
use super::html::{
    HtmlDoc, HtmlElement, HtmlNode, NS_HTML, TAG_LAST_ENTRY as TAG_INDEX_CAP, TAG_UNDEF,
};

pub struct DomIndex {
    /// Every indexed element, grouped by tag id, in document order.
    tag_nodes: Vec<*mut LxbNode>,
    /// `tag_max + 2` entries, or empty.
    tag_off: Vec<usize>,
    /// The highest tag id present.
    tag_max: usize,
    /// Any element with a namespace other than HTML.
    has_foreign: bool,
}

/// An element's tag id, when it is one this index buckets.
#[inline]
fn indexable_tag(el: HtmlElement<'_>) -> Option<usize> {
    let tag = el.node().tag_id();
    (tag != TAG_UNDEF && tag < TAG_INDEX_CAP).then_some(tag)
}

/// Every element under `root` (inclusive), in document order. The walk climbs
/// by parent links rather than recursing, so a deep tree cannot exhaust the
/// stack.
fn elements<'d>(root: HtmlNode<'d>) -> impl Iterator<Item = HtmlElement<'d>> {
    core::iter::successors(Some(root), move |n| n.preorder_next(root)).filter_map(HtmlNode::element)
}

/// Build over `doc`. `None` on allocation failure.
pub(crate) fn build(doc: HtmlDoc<'_>) -> Option<DomIndex> {
    let root = doc.as_node();

    /* Pass 1: one walk to size everything. */
    let mut counts = [0usize; TAG_INDEX_CAP];
    let mut n_indexed = 0usize;
    let mut tag_max = 0usize;
    let mut has_foreign = false;

    for el in elements(root) {
        if el.node().ns_id() != NS_HTML {
            has_foreign = true;
        }
        if let Some(tag) = indexable_tag(el) {
            counts[tag] += 1;
            n_indexed += 1;
            tag_max = tag_max.max(tag);
        }
    }

    let mut idx = DomIndex {
        tag_nodes: Vec::new(),
        tag_off: Vec::new(),
        tag_max,
        has_foreign,
    };

    /* The tag CSR. `cursor` is scratch: a copy of the offsets, advanced as
     * elements are scattered, which is what preserves document order within a
     * bucket. */
    let mut cursor: Vec<usize> = Vec::new();
    if n_indexed > 0 {
        let noff = tag_max.checked_add(2)?;
        idx.tag_off = try_vec_with_capacity(noff)?;
        idx.tag_nodes = try_vec_with_capacity(n_indexed)?;
        cursor = try_vec_with_capacity(noff)?;

        /* Prefix-sum the per-tag counts into offsets. The running sum is
         * checked, not wrapped: a wrapped offset would make a bucket read the
         * wrong slice of tag_nodes. */
        idx.tag_off.push(0);
        let mut running = 0usize;
        for c in counts.iter().take(tag_max + 1) {
            running = running.checked_add(*c)?;
            idx.tag_off.push(running);
        }
        cursor.extend_from_slice(&idx.tag_off); /* reserved above */
        idx.tag_nodes.resize(n_indexed, core::ptr::null_mut());
    }

    /* Pass 2: fill. No failure path - every array is already the right size. */
    for el in elements(root) {
        if let Some(tag) = indexable_tag(el) {
            idx.tag_nodes[cursor[tag]] = el.node().as_raw();
            cursor[tag] += 1;
        }
    }

    Some(idx)
}

/* ------------------------------------------------------------------ *
 * lookups                                                            *
 * ------------------------------------------------------------------ */

impl DomIndex {
    /// The elements with tag id `tag_id`, in document order; empty for a tag
    /// this index does not bucket.
    pub fn tag_bucket(&self, tag_id: usize) -> &[*mut LxbNode] {
        if self.tag_nodes.is_empty()
            || tag_id == TAG_UNDEF
            || tag_id >= TAG_INDEX_CAP
            || tag_id > self.tag_max
        {
            return &[];
        }
        &self.tag_nodes[self.tag_off[tag_id]..self.tag_off[tag_id + 1]]
    }

    /// Whether the document holds any non-HTML element. The `//tag` fast path is
    /// only taken for a document known to be pure HTML.
    pub fn has_foreign(&self) -> bool {
        self.has_foreign
    }
}
