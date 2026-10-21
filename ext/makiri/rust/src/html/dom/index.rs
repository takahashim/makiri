//! The indexes a parsed document builds over its arena on demand
//! ([`HtmlParsed`](super::HtmlParsed) keeps them): tag -> elements for the
//! XPath `//tag` path, and each container's run of descendant text for
//! `Node#text`.
//!
//! Both name nodes by id and text by its span in the byte store, so neither
//! borrows the arena; what keeps them true is that an edit drops them before
//! anything changes (`HtmlParsed::edit`), and that the byte store is
//! compacted - moving every span - only right after (`compact.rs`).

use super::handle::{has_ascii_uppercase, HtmlDoc, HtmlNode, NsId, RawNode, TagId, TAG_LAST_ENTRY};
use super::{Document, DomOom, NodeId, NodeType, Span};
use crate::falloc::{try_vec_with_capacity, VecPush};
use crate::ptr_table::PtrTable;

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
    pub(super) fn build(doc: HtmlDoc<'_>) -> Option<DomIndex> {
        let root = doc.as_node();
        let elements = || root.subtree().filter_map(HtmlNode::element);
        let mut counts = [0usize; TAG_LAST_ENTRY];
        let (mut n, mut tag_max) = (0usize, 0usize);
        let (mut has_foreign, mut has_case_kept) = (false, false);
        for el in elements() {
            let html = el.node().ns_id() == Some(NsId::HTML);
            if !html {
                has_foreign = true;
            }
            if let Some(tag) = el.node().tag_id().and_then(TagId::static_index) {
                counts[tag] += 1;
                n += 1;
                tag_max = tag_max.max(tag);
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
        if n == 0 {
            return Some(idx);
        }
        let noff = tag_max.checked_add(2)?;
        idx.tag_off = try_vec_with_capacity(noff)?;
        idx.tag_nodes = try_vec_with_capacity(n)?;
        let mut cursor: Vec<usize> = try_vec_with_capacity(noff)?;
        idx.tag_off.push(0);
        let mut running = 0usize;
        for c in counts.iter().take(tag_max + 1) {
            running = running.checked_add(*c)?;
            idx.tag_off.push(running);
        }
        cursor.extend_from_slice(&idx.tag_off);
        idx.tag_nodes.resize(n, NodeId::DOCUMENT);
        for el in elements() {
            if let Some(tag) = el.node().tag_id().and_then(TagId::static_index) {
                idx.tag_nodes[cursor[tag]] = el.node().id();
                cursor[tag] += 1;
            }
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

/* ---------- the text index ---------- */

#[derive(Clone, Copy)]
struct Run {
    start: u32,
    end: u32,
}

/// Each element's or fragment's run of descendant text, as the Lexbor
/// adapter's `TextIndex` keeps it - here as spans of the byte store.
pub(super) struct TextIndex {
    slices: Vec<Span>,
    prefix: Vec<usize>,
    runs: PtrTable<Option<RawNode>, Run>,
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

struct Frame<'d> {
    child: Option<HtmlNode<'d>>,
    slot: usize,
}

impl TextIndex {
    /// `Ok(None)`: the index does not fit this document.
    pub(super) fn build(root: HtmlNode<'_>) -> Result<Option<TextIndex>, DomOom> {
        if !is_container(root) {
            return Ok(None);
        }
        let (mut nslices, mut ncont) = (0usize, 0usize);
        for n in root.subtree() {
            if is_container(n) {
                ncont += 1;
            } else if text_span(n).is_some() {
                nslices += 1;
            }
        }
        if nslices > u32::MAX as usize {
            return Ok(None);
        }
        let empty = Run { start: 0, end: 0 };
        let mut t = TextIndex {
            slices: try_vec_with_capacity(nslices).ok_or(DomOom)?,
            prefix: try_vec_with_capacity(nslices + 1).ok_or(DomOom)?,
            runs: PtrTable::with_keys(ncont, empty).ok_or(DomOom)?,
        };
        t.prefix.push(0);
        let mut stack: Vec<Frame<'_>> = try_vec_with_capacity(1).ok_or(DomOom)?;
        let Some(slot) = t.runs.insert(Some(RawNode::from(root)), empty) else {
            return Ok(None);
        };
        stack.push(Frame {
            child: root.first_child(),
            slot,
        });
        while let Some(top) = stack.last_mut() {
            let Some(child) = top.child else {
                let slot = top.slot;
                t.runs.slot_mut(slot).end = t.slices.len() as u32;
                stack.pop();
                continue;
            };
            top.child = child.next();
            if let Some(span) = text_span(child) {
                if t.slices.len() == nslices {
                    return Ok(None);
                }
                let Some(total) = t.prefix[t.slices.len()].checked_add(span.len()) else {
                    return Ok(None);
                };
                t.slices.push(span);
                t.prefix.push(total);
            } else if is_container(child) {
                let start = t.slices.len() as u32;
                let Some(slot) = t
                    .runs
                    .insert(Some(RawNode::from(child)), Run { start, end: start })
                else {
                    return Ok(None);
                };
                stack
                    .falloc_push(Frame {
                        child: child.first_child(),
                        slot,
                    })
                    .map_err(|()| DomOom)?;
            }
        }
        Ok(Some(t))
    }

    pub(super) fn slices_of<'i>(&'i self, doc: &'i Document, node: RawNode) -> Option<TextRun<'i>> {
        let r = self.runs.get(Some(node))?;
        let (start, end) = (r.start as usize, r.end as usize);
        Some(TextRun {
            doc,
            slices: &self.slices[start..end],
            total: self.prefix[end] - self.prefix[start],
        })
    }
}
