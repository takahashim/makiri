//! The indexes a parsed document builds over its arena on demand
//! ([`HtmlParsed`](super::HtmlParsed) keeps them): tag -> elements for the
//! XPath `//tag` path, each container's run of descendant text for
//! `Node#text`, and each node's position in document order for XPath's
//! sorts.
//!
//! Both name nodes by id and text by its span in the byte store, so neither
//! borrows the arena; what keeps them true is that an edit drops them before
//! anything changes (`HtmlParsed::edit`), and that the byte store is
//! compacted - moving every span - only right after (`compact.rs`).

use super::handle::{has_ascii_uppercase, HtmlDoc, HtmlNode, NsId, TagId, TAG_LAST_ENTRY};
use super::{Document, DomOom, NodeId, NodeType, Span};
use falloc::{try_vec_with_capacity, VecGrow};

/* ---------- the element index ---------- */

/// Tag id -> elements, in document order: a CSR layout over the static tag
/// range, as the Lexbor adapter's `DomIndex` is.
pub struct DomIndex {
    tag_nodes: Vec<NodeId>,
    tag_off: Vec<usize>,
    tag_max: usize,
    has_foreign: bool,
    has_case_kept: bool,
}

impl DomIndex {
    /// One walk: each bucketed element's (tag, id) in document order, then
    /// a counting sort of those pairs into the buckets - an array pass, not
    /// a second walk of the tree.
    pub(super) fn build(doc: HtmlDoc<'_>) -> Option<DomIndex> {
        let mut counts = [0usize; TAG_LAST_ENTRY];
        let mut tag_max = 0usize;
        let (mut has_foreign, mut has_case_kept) = (false, false);
        let mut pairs: Vec<(u16, NodeId)> = Vec::new();
        for el in doc.as_node().subtree().filter_map(HtmlNode::element) {
            let html = el.node().ns_id() == Some(NsId::HTML);
            if !html {
                has_foreign = true;
            }
            if let Some(tag) = el.node().tag_id().and_then(TagId::static_index) {
                counts[tag] += 1;
                tag_max = tag_max.max(tag);
                pairs
                    .falloc_push((u16::try_from(tag).ok()?, el.node().id()))
                    .ok()?;
            } else if html && el.has_written_name() && has_ascii_uppercase(el.dom_local_name()) {
                has_case_kept = true;
            }
        }
        let mut idx = DomIndex {
            tag_nodes: Vec::new(),
            tag_off: Vec::new(),
            tag_max,
            has_foreign,
            has_case_kept,
        };
        if pairs.is_empty() {
            return Some(idx);
        }
        let noff = tag_max.checked_add(2)?;
        idx.tag_off = try_vec_with_capacity(noff)?;
        idx.tag_off.falloc_push(0).ok()?;
        let mut running = 0usize;
        for c in counts.iter().take(tag_max + 1) {
            running = running.checked_add(*c)?;
            idx.tag_off.falloc_push(running).ok()?;
        }
        let mut cursor: Vec<usize> = falloc::try_to_vec(&idx.tag_off)?;
        idx.tag_nodes = falloc::try_vec_filled(pairs.len(), NodeId::DOCUMENT)?;
        for (tag, id) in pairs {
            let slot = cursor.get_mut(usize::from(tag))?;
            *idx.tag_nodes.get_mut(*slot)? = id;
            *slot += 1;
        }
        Some(idx)
    }

    /// The elements with tag `tag`, in document order; `None` for a tag not
    /// bucketed (an appended one), `Some(&[])` for a bucketed tag with none.
    pub fn tag_bucket(&self, tag: TagId) -> Option<&[NodeId]> {
        let i = tag.static_index()?;
        if self.tag_nodes.is_empty() || i > self.tag_max {
            return Some(&[]);
        }
        Some(&self.tag_nodes[self.tag_off[i]..self.tag_off[i + 1]])
    }

    /// Whether the document holds any non-HTML element.
    pub fn has_foreign(&self) -> bool {
        self.has_foreign
    }

    /// Whether the document holds an HTML element named with upper case as
    /// written.
    pub fn has_case_kept(&self) -> bool {
        self.has_case_kept
    }
}

/* ---------- the document-order index ---------- */

/// Each node's position in XPath's document order (§5.1): a node, then its
/// attributes in list order, then its children - the order the evaluator's
/// own per-evaluate index numbers (`xpath::order`). Over the document's
/// tree, `<template>` contents left out as the XPath walk leaves them. Every
/// attribute is numbered, those the attribute axis skips (`xmlns:*`)
/// included: only the relative order is read.
///
/// A dense array indexed by `NodeId`, sized to the arena when built: a node
/// outside the tree (detached, in a fragment, in a template's contents), or
/// appended since (`HtmlParsed::edit_keeping_indexes`), has no position.
pub struct DocOrder {
    pre: Vec<u32>,
}

impl DocOrder {
    /// No position.
    const NONE: u32 = u32::MAX;

    /// `None` on OOM, or for an arena too large to number in a `u32`.
    pub(super) fn build(doc: HtmlDoc<'_>, nodes: usize) -> Option<DocOrder> {
        if nodes >= Self::NONE as usize {
            return None;
        }
        let mut pre: Vec<u32> = falloc::try_vec_filled(nodes, Self::NONE)?;
        let mut ord = 0u32;
        let mut number = |n: NodeId| -> Option<()> {
            *pre.get_mut(n.index())? = ord;
            ord = ord.checked_add(1)?;
            Some(())
        };
        for n in doc.as_node().subtree() {
            number(n.id())?;
            if let Some(el) = n.element() {
                for a in el.attrs() {
                    number(a.node().id())?;
                }
            }
        }
        Some(DocOrder { pre })
    }

    /// `node`'s position, or `None` for a node the index does not hold.
    #[inline]
    pub fn position(&self, node: NodeId) -> Option<u32> {
        self.pre
            .get(node.index())
            .copied()
            .filter(|p| *p != Self::NONE)
    }

    /// The bytes it holds, for the GC report.
    pub fn bytes(&self) -> usize {
        self.pre.capacity() * core::mem::size_of::<u32>()
    }
}

/* ---------- the text index ---------- */

#[derive(Clone, Copy)]
struct Run {
    start: u32,
    end: u32,
}

impl Run {
    /// A node the index holds no run for.
    const NONE: Run = Run {
        start: u32::MAX,
        end: u32::MAX,
    };
}

/// Each element's or fragment's run of descendant text: the text spans of
/// the tree in document order, their running byte total, and per container
/// the `[start, end)` of its run - a dense array indexed by `NodeId`, sized
/// to the arena when built, as [`DocOrder`] is.
pub(super) struct TextIndex {
    slices: Vec<Span>,
    prefix: Vec<usize>,
    runs: Vec<Run>,
}

/// The run of text slices one container's subtree owns.
#[derive(Clone, Copy)]
pub struct TextRun<'i> {
    doc: &'i Document,
    slices: &'i [Span],
    total: usize,
}

impl<'i> TextRun<'i> {
    /// The slices, in document order; never empty ones.
    pub fn iter(self) -> impl Iterator<Item = &'i [u8]> {
        let doc = self.doc;
        self.slices.iter().map(move |s| doc.bytes_of(*s))
    }

    /// The byte total of the run.
    pub fn total(self) -> usize {
        self.total
    }
}

fn is_container(n: HtmlNode<'_>) -> bool {
    matches!(
        n.node_type(),
        NodeType::Element | NodeType::DocumentFragment
    )
}

/// The span of a non-empty Text or CDATA node's data.
fn text_span(n: HtmlNode<'_>) -> Option<Span> {
    n.owner_document().arena().text_span(n.id())
}

impl TextIndex {
    /// `Ok(None)`: the index does not fit this document. One walk: the
    /// spans grow as they are met, and each container's run closes when the
    /// walk leaves its subtree.
    pub(super) fn build(root: HtmlNode<'_>, nodes: usize) -> Result<Option<TextIndex>, DomOom> {
        if !is_container(root) {
            return Ok(None);
        }
        let mut t = TextIndex {
            slices: Vec::new(),
            prefix: try_vec_with_capacity(1).ok_or(DomOom)?,
            runs: falloc::try_vec_filled(nodes, Run::NONE).ok_or(DomOom)?,
        };
        t.prefix.falloc_push(0).map_err(|()| DomOom)?;
        let mut n = root;
        loop {
            if let Some(span) = text_span(n) {
                let Some(total) = t.prefix[t.slices.len()].checked_add(span.len()) else {
                    return Ok(None);
                };
                t.slices.falloc_push(span).map_err(|()| DomOom)?;
                t.prefix.falloc_push(total).map_err(|()| DomOom)?;
            } else if is_container(n) {
                let Ok(start) = u32::try_from(t.slices.len()) else {
                    return Ok(None);
                };
                let Some(run) = t.runs.get_mut(n.id().index()) else {
                    return Ok(None);
                };
                *run = Run { start, end: start };
                if let Some(c) = n.first_child() {
                    n = c;
                    continue;
                }
            }
            /* Past `n`'s subtree: close every container the walk leaves. */
            loop {
                if n.id() != root.id() {
                    if let Some(s) = n.next() {
                        n = s;
                        break;
                    }
                }
                let Some(p) = (n.id() != root.id()).then(|| n.parent()).flatten() else {
                    t.close(root);
                    return Ok(Some(t));
                };
                n = p;
                t.close(n);
            }
        }
    }

    /// End `n`'s run at the spans met so far.
    fn close(&mut self, n: HtmlNode<'_>) {
        let end = self.slices.len() as u32;
        if let Some(run) = self.runs.get_mut(n.id().index()) {
            run.end = end;
        }
    }

    pub(super) fn slices_of<'i>(&'i self, doc: &'i Document, node: NodeId) -> Option<TextRun<'i>> {
        let r = self
            .runs
            .get(node.index())
            .filter(|r| r.start != Run::NONE.start)?;
        let (start, end) = (r.start as usize, r.end as usize);
        Some(TextRun {
            doc,
            slices: &self.slices[start..end],
            total: self.prefix[end] - self.prefix[start],
        })
    }
}
