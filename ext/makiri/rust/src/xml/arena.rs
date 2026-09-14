//! The index-arena document: node allocation, the byte store, linking, and the
//! bounded readers the parser and mutators build on.
//!
//! Everything here is safe Rust. A [`Document`] owns a `Vec<Node>` and a
//! `Vec<u8>`; a node is addressed by [`NodeId`], links are `Option<NodeId>`, and
//! names/values are spans into the byte store. No address is ever exposed, so a
//! growing `Vec` cannot invalidate a node, and `detach` (which never destroys)
//! is free to leave a removed node addressable for life.

use crate::falloc::{Reserve, VecPush};
use crate::xml::chars::{expand_into, ExpandErr, ExpandMode};
use crate::xml::{Document, Link, Node, NodeId, NodeType, Span, Status};
use core::sync::atomic::{AtomicU32, Ordering};

/// Hands each document a unique stamp (never 0). Node ids carry it so a handle
/// built for one document is rejected by another's `try_node`.
static DOC_STAMP: AtomicU32 = AtomicU32::new(1);

const NODE_COST: usize = core::mem::size_of::<Node>();

impl Document {
    /// A fresh document with `limits` applied, rejecting `src_len` up front when
    /// it already exceeds the byte budget.
    pub fn create(limits: Option<usize>, src_len: usize) -> Result<Box<Document>, Status> {
        let mut doc = crate::falloc::try_box(Document::blank()).map_err(|_| Status::Oom)?;
        if let Some(mb) = limits {
            if mb != 0 {
                doc.max_bytes = mb;
            }
        }
        if src_len > doc.max_bytes {
            return Err(Status::Limit);
        }
        let mut stamp = DOC_STAMP.fetch_add(1, Ordering::Relaxed);
        if stamp == 0 {
            stamp = DOC_STAMP.fetch_add(1, Ordering::Relaxed);
        }
        doc.stamp = stamp;
        doc.xml_ns = doc.store(crate::xml::XML_NS_URI)?;
        doc.xmlns_ns = doc.store(crate::xml::XMLNS_NS_URI)?;
        /* Index 0 is reserved: it is the null handle's slot (token 0 == a NULL
         * `void *`), so a real node never has index 0. */
        let _null_slot = doc.new_node(NodeType::Document)?;
        doc.doc_node = doc.new_node(NodeType::Document)?;
        Ok(doc)
    }

    #[inline]
    pub fn xml_ns_span(&self) -> Span {
        self.xml_ns
    }
    #[inline]
    pub fn xmlns_ns_span(&self) -> Span {
        self.xmlns_ns
    }

    /// Count `amount` more bytes against the budget, failing closed.
    fn charge(&mut self, amount: usize) -> Result<(), Status> {
        match self.arena_bytes.checked_add(amount) {
            Some(t) if t <= self.max_bytes => {
                self.arena_bytes = t;
                Ok(())
            }
            _ => {
                self.status = Status::Limit;
                Err(Status::Limit)
            }
        }
    }

    /* ---- node access ---- */

    /// The node behind an id the caller KNOWS is live. This is the internal
    /// invariant accessor; at an untrusted boundary (an FFI or engine handle
    /// that could name another document) use [`Document::try_node`], which
    /// fails closed. This one asserts the invariant and is not passed ids of
    /// unknown provenance.
    #[inline]
    pub(crate) fn node(&self, id: NodeId) -> &Node {
        debug_assert_eq!(id.stamp(), self.stamp, "NodeId from another document");
        &self.nodes[id.index() as usize]
    }
    /// As [`Document::node`], for mutation.
    #[inline]
    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        debug_assert_eq!(id.stamp(), self.stamp, "NodeId from another document");
        &mut self.nodes[id.index() as usize]
    }

    /// Resolve a [`Link`] to a handle, re-attaching this document's stamp.
    /// [`Link::NONE`] is no node.
    #[inline]
    fn node_id(&self, l: Link) -> Option<NodeId> {
        if l.is_none() {
            None
        } else {
            Some(self.id_of(l))
        }
    }
    /// The handle a [`Link`] names, re-attaching this document's stamp;
    /// [`Link::NONE`] yields [`NodeId::INVALID`].
    #[inline]
    fn id_of(&self, l: Link) -> NodeId {
        NodeId::new(l.index(), self.stamp)
    }
    /// The node a non-[`Link::NONE`] link names.
    #[inline]
    fn node_at(&self, l: Link) -> &Node {
        &self.nodes[l.index() as usize]
    }
    /// As [`Document::node_at`], for mutation.
    #[inline]
    fn node_at_mut(&mut self, l: Link) -> &mut Node {
        &mut self.nodes[l.index() as usize]
    }

    /// A node that may hold a detached/removed value: `None` for the invalid
    /// handle, a handle from another document, or an out-of-range index.
    #[inline]
    pub fn try_node(&self, id: NodeId) -> Option<&Node> {
        if id.is_invalid() || id.stamp() != self.stamp {
            return None;
        }
        self.nodes.get(id.index() as usize)
    }

    #[inline]
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node_id(self.try_node(id)?.first_child)
    }
    #[inline]
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.node_id(self.try_node(id)?.last_child)
    }
    #[inline]
    pub fn next(&self, id: NodeId) -> Option<NodeId> {
        self.node_id(self.try_node(id)?.next)
    }
    #[inline]
    pub fn prev(&self, id: NodeId) -> Option<NodeId> {
        self.node_id(self.try_node(id)?.prev)
    }
    #[inline]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node_id(self.try_node(id)?.parent)
    }
    #[inline]
    pub fn attrs(&self, id: NodeId) -> Option<NodeId> {
        self.node_id(self.try_node(id)?.attrs)
    }
    #[inline]
    pub fn type_(&self, id: NodeId) -> Option<NodeType> {
        self.try_node(id).map(|n| n.type_)
    }

    /* ---- byte store ---- */

    /// Borrow the bytes a span names. An absent or zero-length span is empty.
    #[inline]
    pub fn span(&self, s: Span) -> &[u8] {
        if s.is_absent() || s.len == 0 {
            return &[];
        }
        &self.bytes[s.off as usize..s.end()]
    }
    #[inline]
    pub fn qname(&self, id: NodeId) -> &[u8] {
        self.try_node(id).map_or(&[], |n| self.span(n.qname))
    }
    #[inline]
    pub fn local(&self, id: NodeId) -> &[u8] {
        self.try_node(id).map_or(&[], |n| self.span(n.local))
    }
    #[inline]
    pub fn prefix(&self, id: NodeId) -> &[u8] {
        self.try_node(id).map_or(&[], |n| self.span(n.prefix))
    }
    #[inline]
    pub fn ns(&self, id: NodeId) -> &[u8] {
        self.try_node(id).map_or(&[], |n| self.span(n.ns_uri))
    }
    #[inline]
    pub fn value(&self, id: NodeId) -> &[u8] {
        self.try_node(id).map_or(&[], |n| self.span(n.value))
    }

    /// Copy `src` into the byte store, returning its span. Empty is the shared
    /// empty span (never an allocation).
    pub fn store(&mut self, src: &[u8]) -> Result<Span, Status> {
        if src.is_empty() {
            // A present-but-empty value: offset is the tail, so it is distinct
            // from the `ABSENT` marker (offset u32::MAX).
            return Ok(Span {
                off: self.bytes.len() as u32,
                len: 0,
            });
        }
        self.charge(src.len())?;
        self.bytes
            .mkr_reserve(src.len())
            .map_err(|_| self.fail(Status::Oom))?;
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(src);
        Ok(Span {
            off,
            len: src.len() as u32,
        })
    }

    /// The empty span, for callers that want a value with no bytes.
    #[inline]
    pub fn empty_span(&self) -> Span {
        Span::EMPTY
    }

    /// Set a node's value to a fresh copy of `data`.
    pub fn set_value_bytes(&mut self, id: NodeId, data: &[u8]) -> Result<(), Status> {
        let span = self.store(data)?;
        self.node_mut(id).value = span;
        Ok(())
    }

    /// Set a node's namespace URI to a fresh copy of `uri`.
    pub fn set_ns_bytes(&mut self, id: NodeId, uri: &[u8]) -> Result<(), Status> {
        let span = self.store(uri)?;
        self.node_mut(id).ns_uri = span;
        Ok(())
    }

    /// Set a leaf's name (PI target) to a fresh copy of `name`.
    pub fn set_local_bytes(&mut self, id: NodeId, name: &[u8]) -> Result<(), Status> {
        let span = self.store(name)?;
        let n = self.node_mut(id);
        n.local = span;
        Ok(())
    }

    /// Copy a whole QName once, then point qname/prefix/local into that copy.
    /// `prefix_len` and `local_off`/`local_len` are offsets into `name`.
    pub fn assign_qname(
        &mut self,
        id: NodeId,
        name: &[u8],
        prefix_len: u32,
        local_off: u32,
        local_len: u32,
    ) -> Result<(), Status> {
        let span = self.store(name)?;
        let n = self.node_mut(id);
        n.qname = span;
        n.prefix = Span {
            off: span.off,
            len: prefix_len,
        };
        n.local = Span {
            off: span.off + local_off,
            len: local_len,
        };
        Ok(())
    }

    /* ---- allocation ---- */

    fn fail(&mut self, st: Status) -> Status {
        self.status = st;
        st
    }

    /// Allocate a zeroed node, counted against the node and byte budgets.
    pub fn new_node(&mut self, type_: NodeType) -> Result<NodeId, Status> {
        if self.nodes.len() + 1 > self.max_nodes {
            return Err(self.fail(Status::Limit));
        }
        self.charge(NODE_COST)?;
        self.nodes
            .mkr_reserve(1)
            .map_err(|_| self.fail(Status::Oom))?;
        let index = self.nodes.len() as u32;
        let stamp = self.stamp;
        self.nodes.push(Node::zeroed(type_));
        Ok(NodeId::new(index, stamp))
    }

    /// Expand XML references into one byte-store span.
    pub fn expand(&mut self, src: &[u8], mode: ExpandMode) -> Result<Span, Status> {
        if src.is_empty() {
            return Ok(Span::EMPTY);
        }
        self.charge(src.len())?;
        self.bytes
            .mkr_reserve(src.len())
            .map_err(|_| self.fail(Status::Oom))?;
        let off = self.bytes.len();
        self.bytes.resize(off + src.len(), 0);
        let n = match expand_into(src, mode, &mut self.bytes[off..]) {
            Ok(n) => n,
            Err(ExpandErr::Syntax) => {
                self.bytes.truncate(off);
                return Err(Status::Syntax);
            }
            Err(ExpandErr::Overflow) => {
                self.bytes.truncate(off);
                return Err(Status::Internal);
            }
        };
        self.bytes.truncate(off + n);
        // The reservation above charged `src.len()`; the expansion never grows
        // (references only shrink), so this is the only accounting needed.
        Ok(Span {
            off: off as u32,
            len: n as u32,
        })
    }

    /// Append a TEXT/CDATA node, coalescing with a preceding sibling of the
    /// SAME type (as libxml2 / the XPath data model do).
    pub fn append_chardata(
        &mut self,
        parent: NodeId,
        type_: NodeType,
        span: Span,
    ) -> Result<(), Status> {
        let last = self.node(parent).last_child;
        if !last.is_none() && self.node_at(last).type_ == type_ {
            let old = self.node_at(last).value;
            if old.end() == span.off as usize {
                // The two chunks are contiguous in the store (the common
                // case): extend the span, no copy.
                let total = (old.len as usize)
                    .checked_add(span.len as usize)
                    .filter(|&t| t <= u32::MAX as usize)
                    .ok_or_else(|| self.fail(Status::Limit))?;
                self.node_at_mut(last).value.len = total as u32;
                return Ok(());
            }
            /* Not contiguous: rebuild the coalesced bytes once. */
            let (a, b) = (old, span);
            let mut merged: Vec<u8> = Vec::new();
            merged
                .mkr_extend(self.span(a))
                .and_then(|()| merged.mkr_extend(self.span(b)))
                .map_err(|_| self.fail(Status::Oom))?;
            let s = self.store(&merged)?;
            self.node_at_mut(last).value = s;
            return Ok(());
        }
        let node = self.new_node(type_)?;
        self.node_mut(node).value = span;
        self.append_child(parent, node);
        Ok(())
    }

    /* ---- linking ---- */

    #[inline]
    pub fn set_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.node_mut(id).parent = Link::from_option(parent);
    }

    /// Append `child` as the last child of `parent`.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        let (parent, child) = (Link::of(parent), Link::of(child));
        self.node_at_mut(child).parent = parent;
        let last = self.node_at(parent).last_child;
        if last.is_none() {
            self.node_at_mut(parent).first_child = child;
        } else {
            self.node_at_mut(last).next = child;
            self.node_at_mut(child).prev = last;
        }
        self.node_at_mut(parent).last_child = child;
    }

    /// Unlink `node` from its parent (child chain or attribute chain). No-op
    /// when the node is already detached.
    pub fn detach(&mut self, node: NodeId) {
        let node_link = Link::of(node);
        let parent = self.node_at(node_link).parent;
        if parent.is_none() {
            return;
        }
        if self.node_at(node_link).type_ == NodeType::Attribute {
            let mut prev = Link::NONE;
            let mut attr = self.node_at(parent).attrs;
            while !attr.is_none() {
                if attr == node_link {
                    self.unlink_attr(self.id_of(parent), self.node_id(prev), self.id_of(attr));
                    break;
                }
                prev = attr;
                attr = self.node_at(attr).next;
            }
            self.clear_links(node_link);
            return;
        }
        let (prev, next) = (self.node_at(node_link).prev, self.node_at(node_link).next);
        if prev.is_none() {
            self.node_at_mut(parent).first_child = next;
        } else {
            self.node_at_mut(prev).next = next;
        }
        if next.is_none() {
            self.node_at_mut(parent).last_child = prev;
        } else {
            self.node_at_mut(next).prev = prev;
        }
        self.clear_links(node_link);
    }

    #[inline]
    fn clear_links(&mut self, node: Link) {
        let n = self.node_at_mut(node);
        n.parent = Link::NONE;
        n.prev = Link::NONE;
        n.next = Link::NONE;
    }

    /// Unlink attribute `a` (predecessor `prev`, `None` if head) from `el`.
    pub fn unlink_attr(&mut self, el: NodeId, prev: Option<NodeId>, a: NodeId) {
        let next = self.node(a).next;
        match prev {
            Some(p) => self.node_mut(p).next = next,
            None => self.node_mut(el).attrs = next,
        }
        self.clear_links(Link::of(a));
    }

    /// Append `attr` to `el`'s attribute list.
    pub fn append_attr(&mut self, el: NodeId, attr: NodeId) {
        self.node_mut(attr).parent = Link::of(el);
        let head = self.node(el).attrs;
        if head.is_none() {
            self.node_mut(el).attrs = Link::of(attr);
        } else {
            let mut t = head;
            while !self.node_at(t).next.is_none() {
                t = self.node_at(t).next;
            }
            self.node_at_mut(t).next = Link::of(attr);
        }
    }

    /// The ONE place the doubly-linked child list is written by insertion.
    pub fn splice_between(
        &mut self,
        container: NodeId,
        node: NodeId,
        prev: Option<NodeId>,
        next: Option<NodeId>,
    ) {
        let container = Link::of(container);
        let node = Link::of(node);
        let prev = Link::from_option(prev);
        let next = Link::from_option(next);
        {
            let n = self.node_at_mut(node);
            n.parent = container;
            n.prev = prev;
            n.next = next;
        }
        if prev.is_none() {
            self.node_at_mut(container).first_child = node;
        } else {
            self.node_at_mut(prev).next = node;
        }
        if next.is_none() {
            self.node_at_mut(container).last_child = node;
        } else {
            self.node_at_mut(next).prev = node;
        }
    }

    /* ---- tree walks ---- */

    /// Pre-order (document-order) successor of `cur` within `root`'s subtree.
    pub fn preorder_next(&self, root: NodeId, cur: NodeId) -> Option<NodeId> {
        let root_link = Link::of(root);
        let mut cur_link = Link::of(cur);
        let first = self.node_at(cur_link).first_child;
        if !first.is_none() {
            return self.node_id(first);
        }
        while cur_link != root_link && self.node_at(cur_link).next.is_none() {
            let up = self.node_at(cur_link).parent;
            if up.is_none() {
                return None;
            }
            cur_link = up;
        }
        if cur_link == root_link {
            return None;
        }
        self.node_id(self.node_at(cur_link).next)
    }

    /// `node`'s topmost ancestor is the document node.
    pub fn is_connected(&self, node: NodeId) -> bool {
        let mut top = Link::of(node);
        while !self.node_at(top).parent.is_none() {
            top = self.node_at(top).parent;
        }
        self.node_at(top).type_ == NodeType::Document
    }

    /// Nearest in-scope binding for `prefix` ("" = default) at or above `node`.
    pub fn resolve_in_scope(&self, node: Option<NodeId>, prefix: &[u8]) -> Option<Span> {
        let mut e = node.map(Link::of);
        while let Some(id) = e {
            if self.node_at(id).type_ == NodeType::Element {
                let mut a = self.node_at(id).attrs;
                while !a.is_none() {
                    if let Some(p) =
                        crate::xml::qname::xmlns_prefix(self.span(self.node_at(a).qname))
                    {
                        if p == prefix {
                            return Some(self.node_at(a).value);
                        }
                    }
                    a = self.node_at(a).next;
                }
            }
            e = self.node_at(id).parent.optional();
        }
        None
    }

    /// True when two attributes share `(local name, namespace URI)`.
    pub fn has_duplicate_attributes(&self, element: NodeId) -> bool {
        let mut a = self.node(element).attrs;
        while !a.is_none() {
            let mut b = self.node_at(a).next;
            while !b.is_none() {
                if self.span(self.node_at(a).local) == self.span(self.node_at(b).local)
                    && self.span(self.node_at(a).ns_uri) == self.span(self.node_at(b).ns_uri)
                {
                    return true;
                }
                b = self.node_at(b).next;
            }
            a = self.node_at(a).next;
        }
        false
    }

    /* ---- document meta ---- */

    #[inline]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }
    #[inline]
    pub fn set_root(&mut self, root: Option<NodeId>) {
        self.root = root;
    }
    #[inline]
    pub fn doctype(&self) -> Option<NodeId> {
        self.doctype
    }
    #[inline]
    pub fn set_doctype(&mut self, doctype: Option<NodeId>) {
        self.doctype = doctype;
    }
    #[inline]
    pub fn doc_node(&self) -> NodeId {
        self.doc_node
    }
    #[inline]
    pub fn mark_encoding_decl(&mut self) {
        self.has_encoding_decl = true;
    }

    /// Re-derive root / doctype from the tree after a change at the document
    /// node.
    pub fn sync_doc_meta(&mut self, container: NodeId) {
        if container != self.doc_node {
            return;
        }
        self.root = None;
        self.doctype = None;
        let mut c = self.node(self.doc_node).first_child;
        while !c.is_none() {
            let t = self.node_at(c).type_;
            if self.root.is_none() && t == NodeType::Element {
                self.root = Some(self.id_of(c));
            }
            if self.doctype.is_none() && t == NodeType::Doctype {
                self.doctype = Some(self.id_of(c));
            }
            c = self.node_at(c).next;
        }
    }

    /// Total bytes the document holds, for `Document#memsize`.
    pub fn memsize(&self) -> usize {
        let mut total = core::mem::size_of::<Document>();
        let extra = self
            .nodes
            .capacity()
            .saturating_mul(NODE_COST)
            .saturating_add(self.bytes.capacity());
        match total.checked_add(extra) {
            Some(t) => total = t,
            None => return usize::MAX,
        }
        total
    }
}

/// The C `""` sentinel check: a span is empty. Kept for the FFI adapter.
#[inline]
pub fn span_is_empty(s: Span) -> bool {
    s.len == 0
}
