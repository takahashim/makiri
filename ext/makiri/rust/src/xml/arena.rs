//! The index-arena document: node allocation, the byte store, linking, and the
//! bounded readers the parser and mutators build on.
//!
//! Everything here is safe Rust. A [`Document`] owns a `Vec<Node>` and a
//! `Vec<u8>`; a node is addressed by [`NodeId`], links are `Option<NodeId>`, and
//! names/values are spans into the byte store. No address is ever exposed, so a
//! growing `Vec` cannot invalidate a node, and `detach` (which never destroys)
//! is free to leave a removed node addressable for life.
//!
//! # Two visibilities, on purpose
//!
//! The READERS are `pub`: a `NodeId` cannot name the wrong document, because
//! [`Document::try_node`] checks its stamp, so handing them out is safe. The
//! byte and link SURGERY - `store`, `new_node`, `append_child`, `detach`,
//! `splice_between`, `sync_doc_meta` and the rest - is `pub(super)`, which from
//! here means "inside `crate::xml`". It skips every rule `mutate` enforces
//! (hierarchy, cycles, namespace resolution, index invalidation), so the glue
//! and the bridge must not be able to reach it; before, it was `pub` like the
//! readers and only convention kept them apart.

#![forbid(unsafe_code)]

use crate::falloc::{Reserve, VecPush};
use crate::xml::chars::{expand_into, ExpandErr, ExpandMode};
use crate::xml::qname::Split;
use crate::xml::{Document, Link, Node, NodeId, NodeType, Span, Status};
use core::sync::atomic::{AtomicU32, Ordering};

/// Hands each document a unique stamp (never 0). Node ids carry it so a handle
/// built for one document is rejected by another's `try_node`.
///
/// It wraps after 2^32 documents, so in principle a handle kept across that many
/// later parses could match a stamp again. That is not a hazard worth widening
/// the counter for: a `NodeId` is only ever obtained from a live Ruby wrapper,
/// which keeps its document alive, so a handle and its document cannot drift
/// four billion documents apart - and a handle from a document that is GONE has
/// no way to reach `try_node` at all.
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
            _ => Err(Status::Limit),
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

    /* ---- names and ids, by node kind ----
     *
     * A DOCTYPE repurposes the name fields - `prefix` holds its PUBLIC id and
     * `value` its SYSTEM id - so a reader that takes those without asking the
     * kind reads an id as a name. These two answer by kind; the raw accessors
     * above are for code that has already established it. */

    /// An element's or attribute's naming, or None for any other kind.
    pub fn name_parts(&self, id: NodeId) -> Option<NameParts<'_>> {
        let n = self.try_node(id)?;
        let present = |s: Span| (s.len != 0).then(|| self.span(s));
        matches!(n.type_, NodeType::Element | NodeType::Attribute).then(|| NameParts {
            qname: self.span(n.qname),
            local: self.span(n.local),
            prefix: present(n.prefix),
            ns_uri: present(n.ns_uri),
        })
    }

    /// A DOCTYPE's PUBLIC and SYSTEM ids, or None for any other kind.
    pub fn doctype_ids(&self, id: NodeId) -> Option<DoctypeIds<'_>> {
        let n = self.try_node(id)?;
        let written = |s: Span| (!s.is_absent()).then(|| self.span(s));
        (n.type_ == NodeType::Doctype).then(|| DoctypeIds {
            public: written(n.prefix),
            system: written(n.value),
        })
    }

    /// Copy `src` into the byte store, returning its span. Empty is the shared
    /// empty span (never an allocation).
    pub(super) fn store(&mut self, src: &[u8]) -> Result<Span, Status> {
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
            .falloc_reserve(src.len())
            .map_err(|_| Status::Oom)?;
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(src);
        Ok(Span {
            off,
            len: src.len() as u32,
        })
    }

    /// Set a node's value to a fresh copy of `data`.
    pub(super) fn set_value_bytes(&mut self, id: NodeId, data: &[u8]) -> Result<(), Status> {
        let span = self.store(data)?;
        self.node_mut(id).value = span;
        Ok(())
    }

    /// Set a node's namespace URI to a fresh copy of `uri`.
    pub(super) fn set_ns_bytes(&mut self, id: NodeId, uri: &[u8]) -> Result<(), Status> {
        let span = self.store(uri)?;
        self.node_mut(id).ns_uri = span;
        Ok(())
    }

    /// The prefix/local split of `id`'s qualified name.
    ///
    /// The three name spans all point into ONE arena copy (see
    /// [`Document::assign_qname`]), so the split is derived from their offsets
    /// rather than stored - and derived HERE, not at each of the four callers
    /// that used to recompute `local.off - qname.off` by hand.
    pub(crate) fn split_of(&self, id: NodeId) -> Split {
        let n = self.node(id);
        Split {
            prefix_len: n.prefix.len,
            local_off: n.local.off.saturating_sub(n.qname.off),
            local_len: n.local.len,
        }
    }

    /// Copy a whole QName once, then point qname/prefix/local into that copy.
    /// `prefix_len` and `local_off`/`local_len` are offsets into `name`.
    pub(super) fn assign_qname(
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

    /// Allocate a zeroed node, counted against the node and byte budgets.
    pub(super) fn new_node(&mut self, type_: NodeType) -> Result<NodeId, Status> {
        if self.nodes.len() + 1 > self.max_nodes {
            return Err(Status::Limit);
        }
        self.charge(NODE_COST)?;
        self.nodes.falloc_reserve(1).map_err(|_| Status::Oom)?;
        let index = self.nodes.len() as u32;
        let stamp = self.stamp;
        self.nodes.push(Node::zeroed(type_));
        Ok(NodeId::new(index, stamp))
    }

    /// Expand XML references into one byte-store span.
    pub(super) fn expand(&mut self, src: &[u8], mode: ExpandMode) -> Result<Span, Status> {
        if src.is_empty() {
            return Ok(Span::EMPTY);
        }
        self.charge(src.len())?;
        self.bytes
            .falloc_reserve(src.len())
            .map_err(|_| Status::Oom)?;
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
    pub(super) fn append_chardata(
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
                    .ok_or(Status::Limit)?;
                self.node_at_mut(last).value.len = total as u32;
                return Ok(());
            }
            /* Not contiguous: rebuild the coalesced bytes once. */
            let (a, b) = (old, span);
            let mut merged: Vec<u8> = Vec::new();
            merged
                .falloc_extend(self.span(a))
                .and_then(|()| merged.falloc_extend(self.span(b)))
                .map_err(|_| Status::Oom)?;
            let s = self.store(&merged)?;
            self.node_at_mut(last).value = s;
            return Ok(());
        }
        let node = self.new_node(type_)?;
        self.node_mut(node).value = span;
        self.append_child(parent, node);
        Ok(())
    }

    /* ---- rewind ---- */

    /// The arena's current high-water mark, for [`Document::rewind`].
    #[inline]
    pub(crate) fn mark(&self) -> Mark {
        Mark {
            nodes: self.nodes.len(),
            bytes: self.bytes.len(),
            arena_bytes: self.arena_bytes,
        }
    }

    /// Discard everything allocated since `mark`, giving the budget back.
    ///
    /// Sound only when NOTHING allocated after the mark escaped the caller and
    /// nothing allocated before it points past the mark - i.e. the work being
    /// undone was never linked into the live tree and never handed out as a
    /// handle. A failed fragment parse is exactly that case: its nodes hang off
    /// a fragment root the caller never returns.
    pub(crate) fn rewind(&mut self, mark: Mark) {
        debug_assert!(mark.nodes <= self.nodes.len() && mark.bytes <= self.bytes.len());
        self.nodes.truncate(mark.nodes);
        self.bytes.truncate(mark.bytes);
        self.arena_bytes = mark.arena_bytes;
    }

    /* ---- linking ---- */

    #[inline]
    pub(super) fn set_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.node_mut(id).parent = Link::from_option(parent);
    }

    /// Append `child` as the last child of `parent`.
    pub(super) fn append_child(&mut self, parent: NodeId, child: NodeId) {
        let (parent, child) = (Link::of(parent), Link::of(child));
        assert_no_self_link(child, parent, self.node_at(parent).last_child, Link::NONE);
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
    pub(super) fn detach(&mut self, node: NodeId) {
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
            self.clear_links_at(node_link);
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
        self.clear_links_at(node_link);
    }

    /// Forget `node`'s parent and siblings, leaving its CHILDREN alone.
    ///
    /// `pub(super)` because `mutate` needs it for the node a `replace` swapped
    /// out: that node's links were already taken over by the splice, so
    /// `detach` would unlink the wrong thing. Two callers there open-coded the
    /// three assignments through `node_mut`, which is the layer this module's
    /// visibility split exists to close.
    #[inline]
    pub(super) fn clear_links(&mut self, node: NodeId) {
        self.clear_links_at(Link::of(node));
    }

    #[inline]
    fn clear_links_at(&mut self, node: Link) {
        let n = self.node_at_mut(node);
        n.parent = Link::NONE;
        n.prev = Link::NONE;
        n.next = Link::NONE;
    }

    /// Detach every child of `node` and make `only` its single child (or leave
    /// it childless when `only` is None).
    ///
    /// One arena operation because it is one invariant: every former child ends
    /// up fully unlinked AND `first_child`/`last_child` agree with what is
    /// actually there. `set_content` wrote both halves by hand.
    pub(super) fn replace_children(&mut self, node: NodeId, only: Option<NodeId>) {
        let mut c = self.first_child(node);
        while let Some(cur) = c {
            let next = self.next(cur);
            self.clear_links(cur);
            c = next;
        }
        {
            let n = self.node_mut(node);
            n.first_child = Link::from_option(only);
            n.last_child = Link::from_option(only);
        }
        if let Some(only) = only {
            self.set_parent(only, Some(node));
        }
    }

    /// Unlink attribute `a` (predecessor `prev`, `None` if head) from `el`.
    pub(super) fn unlink_attr(&mut self, el: NodeId, prev: Option<NodeId>, a: NodeId) {
        let next = self.node(a).next;
        match prev {
            Some(p) => self.node_mut(p).next = next,
            None => self.node_mut(el).attrs = next,
        }
        self.clear_links_at(Link::of(a));
    }

    /// Link `attr` onto `el`'s attribute list after `tail`, the list's current
    /// last entry (`None` when the list is empty).
    ///
    /// The ONLY way to extend the list, and it takes the tail rather than
    /// finding it: every caller has just scanned the list - looking for an
    /// attribute of the same name, or building the list in order - so it already
    /// knows the end. An `append_attr` that walked to it existed and turned out
    /// to have no callers left once the tail was threaded through.
    pub(super) fn link_attr(&mut self, el: NodeId, tail: Option<NodeId>, attr: NodeId) {
        assert_no_self_link(
            Link::of(attr),
            Link::of(el),
            Link::from_option(tail),
            Link::NONE,
        );
        self.node_mut(attr).parent = Link::of(el);
        match tail {
            None => self.node_mut(el).attrs = Link::of(attr),
            Some(t) => self.node_mut(t).next = Link::of(attr),
        }
    }

    /// The ONE place the doubly-linked child list is written by insertion.
    pub(super) fn splice_between(
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
        assert_no_self_link(node, container, prev, next);
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
    ///
    /// `cur` is checked, not assumed: this is reached from the Ruby glue with a
    /// handle a wrapper supplied, so a stale or foreign one must answer "no
    /// successor" rather than read whatever slot its index lands on. The link
    /// walk after that needs no check - a link always names a live slot of THIS
    /// document.
    pub fn preorder_next(&self, root: NodeId, cur: NodeId) -> Option<NodeId> {
        self.try_node(cur)?;
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

    /// `node`'s topmost ancestor is the document node. Internal: it walks links
    /// unchecked, so the caller must hold a live handle.
    pub(crate) fn is_connected(&self, node: NodeId) -> bool {
        let mut top = Link::of(node);
        while !self.node_at(top).parent.is_none() {
            top = self.node_at(top).parent;
        }
        self.node_at(top).type_ == NodeType::Document
    }

    /* ---- document meta ---- */

    #[inline]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }
    #[inline]
    pub(super) fn set_root(&mut self, root: Option<NodeId>) {
        self.root = root;
    }
    #[inline]
    pub fn doctype(&self) -> Option<NodeId> {
        self.doctype
    }
    #[inline]
    pub(super) fn set_doctype(&mut self, doctype: Option<NodeId>) {
        self.doctype = doctype;
    }
    #[inline]
    pub fn doc_node(&self) -> NodeId {
        self.doc_node
    }
    #[inline]
    pub(super) fn mark_encoding_decl(&mut self) {
        self.has_encoding_decl = true;
    }

    /// Re-derive root / doctype from the tree after a change at the document
    /// node.
    pub(super) fn sync_doc_meta(&mut self, container: NodeId) {
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

/// A node may not be its own parent or its own sibling.
///
/// Checked in RELEASE at the three places that write a link, which is not the
/// usual `debug_assert` trade. A cycle here is not a wrong answer that a later
/// check could catch: `node.next == node` is a ring, and every walk in the
/// engine follows `next` without a bound, so the first traversal afterwards
/// hangs the host process with no way out. Two `u32` compares against an
/// unrecoverable hang is not a close call, and turning a broken invariant into a
/// panic (which `bridge::ruby::entry` presents as `Makiri::InternalError`) is
/// what this codebase does with broken invariants everywhere else.
///
/// It is also what makes the mutation fuzzer safe to run in CI: a ring cannot be
/// created, so no generated edit sequence can hang the suite. The bug that
/// prompted this (`a.add_next_sibling(b)` with b already after a, spliced before
/// ITSELF) reached exactly here, as `node == next`.
#[inline]
#[allow(
    clippy::panic,
    reason = "a sibling ring hangs the process; a panic is the recoverable outcome"
)]
fn assert_no_self_link(node: Link, container: Link, prev: Link, next: Link) {
    if node == container || node == prev || node == next {
        panic!("XML arena: a node cannot be its own parent or sibling");
    }
}

/// An arena high-water mark: what [`Document::rewind`] restores.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mark {
    nodes: usize,
    bytes: usize,
    arena_bytes: usize,
}

/// How an element or attribute is named: [`Document::name_parts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameParts<'a> {
    pub qname: &'a [u8],
    pub local: &'a [u8],
    /// None when unprefixed.
    pub prefix: Option<&'a [u8]>,
    /// None when in no namespace.
    pub ns_uri: Option<&'a [u8]>,
}

/// A DOCTYPE's identifiers: [`Document::doctype_ids`]. An omitted id is None;
/// one written as `""` is `Some(b"")`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoctypeIds<'a> {
    pub public: Option<&'a [u8]>,
    pub system: Option<&'a [u8]>,
}
