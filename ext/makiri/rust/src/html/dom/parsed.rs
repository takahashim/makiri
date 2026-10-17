//! A parsed arena document, as a Ruby Document holds it: the [`Document`], its
//! line table, and the indexes built over it on demand - the counterpart of the
//! Lexbor adapter's `post_parse::HtmlParsed`, with the same methods.
//!
//! The indexes name nodes by id and text by its place in the byte store, so
//! nothing here borrows across a change: an edit takes `&mut` of the whole
//! handle ([`HtmlParsed::edit`]), which drops the indexes first.

use core::ptr::NonNull;

use super::edit::HtmlDocMut;
use super::handle::{has_ascii_uppercase, HtmlDoc, HtmlNode, NsId, RawNode, TagId, TAG_LAST_ENTRY};
use super::{Document, DomOom, NodeId, NodeType, Span};
use crate::falloc::{try_vec_with_capacity, VecPush};
use crate::ptr_table::PtrTable;
use crate::source_lines::Lines;

/// The identity of a live parsed document: the address of its
/// [`HtmlParsed`], stable for as long as the Ruby Document that owns it.
/// Compare-only.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HtmlDocIdentity(NonNull<HtmlParsed>);

/// A node that is not a node of the document it was keyed or resolved
/// against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ForeignNode;

/// A long-lived node handle: the node and the identity of its document.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HtmlNodeKey {
    owner: HtmlDocIdentity,
    node: RawNode,
}

impl HtmlNodeKey {
    /// The node, for the bridge's node identity and transport.
    #[inline]
    pub fn raw_node(self) -> RawNode {
        self.node
    }
}

/// A parsed HTML document, and the indexes built over it on demand.
pub struct HtmlParsed {
    doc: Document,
    dom_index: Option<DomIndex>,
    lines: Option<Lines>,
    text_index: TextIndexState,
}

enum TextIndexState {
    Unbuilt,
    Built(TextIndex),
    Inapplicable,
}

impl HtmlParsed {
    /// A parsed document, with its line table when there is one.
    pub fn new(doc: Document, lines: Option<Lines>) -> Self {
        HtmlParsed {
            doc,
            dom_index: None,
            lines,
            text_index: TextIndexState::Unbuilt,
        }
    }

    /// The document, for reading.
    #[inline]
    pub fn doc(&self) -> HtmlDoc<'_> {
        HtmlDoc::of(&self.doc)
    }

    /// The document, lent for an edit. The indexes are dropped first, so no
    /// index outlives the tree it describes.
    #[inline]
    pub fn edit(&mut self) -> HtmlDocMut<'_> {
        self.invalidate_indexes();
        HtmlDocMut::new(&mut self.doc)
    }

    /// The arena itself.
    #[inline]
    pub fn arena(&self) -> &Document {
        &self.doc
    }

    /// Build the tag -> elements index if it is not built. `Err` when the
    /// build cannot allocate, which caches nothing.
    pub fn ensure_dom_index(&mut self) -> Result<(), DomOom> {
        if self.dom_index.is_none() {
            self.dom_index = Some(DomIndex::build(self.doc()).ok_or(DomOom)?);
        }
        Ok(())
    }

    /// The tag -> elements index, once built.
    pub fn dom_index(&self) -> Option<&DomIndex> {
        self.dom_index.as_ref()
    }

    /// The elements with tag `tag`, in document order, by id; `None` until
    /// the index is built, and for a tag it does not bucket.
    pub fn tag_bucket(&self, tag: TagId) -> Option<&[NodeId]> {
        self.dom_index.as_ref()?.tag_bucket(tag)
    }

    /// Build the node -> text index if it is not built and can be.
    pub fn ensure_text_index(&mut self) -> Result<(), DomOom> {
        if matches!(self.text_index, TextIndexState::Unbuilt) {
            let Some(root) = self.doc().as_node().document_root() else {
                self.text_index = TextIndexState::Inapplicable;
                return Ok(());
            };
            self.text_index = match TextIndex::build(root)? {
                Some(t) => TextIndexState::Built(t),
                None => TextIndexState::Inapplicable,
            };
        }
        Ok(())
    }

    /// The run of text slices `node`'s subtree owns, or `None`: walk
    /// instead.
    pub fn text_slices(&self, node: RawNode) -> Option<TextRun<'_>> {
        match &self.text_index {
            TextIndexState::Built(index) => index.slices_of(&self.doc, node),
            _ => None,
        }
    }

    /// Drop the indexes so the next query rebuilds them.
    pub fn invalidate_indexes(&mut self) {
        self.dom_index = None;
        self.text_index = TextIndexState::Unbuilt;
    }

    /// The bytes this document holds outside Ruby's allocator.
    pub fn external_bytes(&self) -> usize {
        self.doc.owned_bytes()
    }

    /// An O(1) measure that grows with the document, for deciding when to
    /// measure [`external_bytes`](Self::external_bytes) again: the node count
    /// plus the byte store in 4 KiB pages.
    pub fn arena_chunks(&self) -> usize {
        self.doc.node_count() / 64 + self.doc.bytes.len() / 4096
    }

    /// The 1-based source line for `node`, or `None` when unknown.
    pub fn node_line(&self, node: RawNode) -> Option<usize> {
        let lines = self.lines.as_ref()?;
        let offset = self.doc().get(node.id())?.source_offset()?;
        Some(lines.lookup(offset))
    }

    /// The identity of this document, for a node key.
    #[inline]
    pub fn identity(&self) -> HtmlDocIdentity {
        HtmlDocIdentity(NonNull::from(self))
    }

    /// The key for `node`, which must be a node of this document.
    pub fn mint_key(&self, node: RawNode) -> Result<HtmlNodeKey, ForeignNode> {
        if self.doc().get(node.id()).is_none() {
            return Err(ForeignNode);
        }
        Ok(HtmlNodeKey {
            owner: self.identity(),
            node,
        })
    }

    /// The node `key` names, when it belongs to this document.
    pub fn resolve(&self, key: HtmlNodeKey) -> Result<HtmlNode<'_>, ForeignNode> {
        if key.owner != self.identity() {
            return Err(ForeignNode);
        }
        self.doc().get(key.node.id()).ok_or(ForeignNode)
    }
}

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
    fn build(doc: HtmlDoc<'_>) -> Option<DomIndex> {
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
struct TextIndex {
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
    if !matches!(n.node_type(), NodeType::Text | NodeType::CDataSection) {
        return None;
    }
    match n.owner_document().arena().node(n.id())?.payload {
        super::Payload::Data(s) if s.len > 0 => Some(s),
        _ => None,
    }
}

struct Frame<'d> {
    child: Option<HtmlNode<'d>>,
    slot: usize,
}

impl TextIndex {
    /// `Ok(None)`: the index does not fit this document.
    fn build(root: HtmlNode<'_>) -> Result<Option<TextIndex>, DomOom> {
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
                let Some(total) = t.prefix[t.slices.len()].checked_add(span.len as usize) else {
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

    fn slices_of<'i>(&'i self, doc: &'i Document, node: RawNode) -> Option<TextRun<'i>> {
        let r = self.runs.get(Some(node))?;
        let (start, end) = (r.start as usize, r.end as usize);
        Some(TextRun {
            doc,
            slices: &self.slices[start..end],
            total: self.prefix[end] - self.prefix[start],
        })
    }
}
