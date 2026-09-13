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
use crate::xml::{
    Doc, Document, Limits, Node, NodeId, Span, ERR_INTERNAL, ERR_LIMIT, ERR_OOM, ERR_SYNTAX,
    T_ATTRIBUTE, T_CDATA, T_COMMENT, T_DOCTYPE, T_DOCUMENT, T_ELEMENT, T_FRAGMENT, T_PI, T_TEXT,
};
use core::ffi::c_char;
use core::sync::atomic::{AtomicU32, Ordering};

/// Hands each document a unique stamp (never 0). Node ids carry it so a handle
/// built for one document is rejected by another's `try_node`.
static DOC_STAMP: AtomicU32 = AtomicU32::new(1);

#[inline]
fn valid_type(t: u32) -> bool {
    matches!(
        t,
        T_ELEMENT
            | T_ATTRIBUTE
            | T_TEXT
            | T_CDATA
            | T_PI
            | T_COMMENT
            | T_DOCUMENT
            | T_DOCTYPE
            | T_FRAGMENT
    )
}

const NODE_COST: usize = core::mem::size_of::<Node>();

impl Document {
    /// A fresh document with `limits` applied, rejecting `src_len` up front when
    /// it already exceeds the byte budget.
    pub fn create(limits: Option<usize>, src_len: usize) -> Result<Box<Document>, i32> {
        let mut doc = crate::falloc::try_box(Document::blank()).map_err(|_| ERR_OOM)?;
        if let Some(mb) = limits {
            if mb != 0 {
                doc.max_bytes = mb;
            }
        }
        if src_len > doc.max_bytes {
            return Err(ERR_LIMIT);
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
        let _null_slot = doc.new_node(T_DOCUMENT)?;
        doc.doc_node = doc.new_node(T_DOCUMENT)?;
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

    #[inline]
    pub fn status(&self) -> i32 {
        self.oom
    }

    /// Count `amount` more bytes against the budget, failing closed.
    fn charge(&mut self, amount: usize) -> Result<(), i32> {
        match self.arena_bytes.checked_add(amount) {
            Some(t) if t <= self.max_bytes => {
                self.arena_bytes = t;
                Ok(())
            }
            _ => {
                self.oom = ERR_LIMIT;
                Err(ERR_LIMIT)
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
        let n = &self.nodes[id.index() as usize];
        debug_assert_eq!(
            n.generation,
            id.generation(),
            "NodeId from another document"
        );
        n
    }
    /// As [`Document::node`], for mutation.
    #[inline]
    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        let n = &mut self.nodes[id.index() as usize];
        debug_assert_eq!(
            n.generation,
            id.generation(),
            "NodeId from another document"
        );
        n
    }

    /// A node that may hold a detached/removed value: `None` for the invalid
    /// handle or a stale generation.
    #[inline]
    pub fn try_node(&self, id: NodeId) -> Option<&Node> {
        if id.is_invalid() {
            return None;
        }
        self.nodes
            .get(id.index() as usize)
            .filter(|n| n.generation == id.generation())
    }

    #[inline]
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.try_node(id).and_then(|n| n.first_child)
    }
    #[inline]
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.try_node(id).and_then(|n| n.last_child)
    }
    #[inline]
    pub fn next(&self, id: NodeId) -> Option<NodeId> {
        self.try_node(id).and_then(|n| n.next)
    }
    #[inline]
    pub fn prev(&self, id: NodeId) -> Option<NodeId> {
        self.try_node(id).and_then(|n| n.prev)
    }
    #[inline]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.try_node(id).and_then(|n| n.parent)
    }
    #[inline]
    pub fn attrs(&self, id: NodeId) -> Option<NodeId> {
        self.try_node(id).and_then(|n| n.attrs)
    }
    #[inline]
    pub fn type_(&self, id: NodeId) -> u32 {
        self.try_node(id).map_or(0, |n| n.type_)
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
    pub fn store(&mut self, src: &[u8]) -> Result<Span, i32> {
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
            .map_err(|_| self.fail(ERR_OOM))?;
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
    pub fn set_value_bytes(&mut self, id: NodeId, data: &[u8]) -> Result<(), i32> {
        let span = self.store(data)?;
        self.node_mut(id).value = span;
        Ok(())
    }

    /// Set a node's namespace URI to a fresh copy of `uri`.
    pub fn set_ns_bytes(&mut self, id: NodeId, uri: &[u8]) -> Result<(), i32> {
        let span = self.store(uri)?;
        self.node_mut(id).ns_uri = span;
        Ok(())
    }

    /// Set a leaf's name (PI target) to a fresh copy of `name`.
    pub fn set_local_bytes(&mut self, id: NodeId, name: &[u8]) -> Result<(), i32> {
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
    ) -> Result<(), i32> {
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

    fn fail(&mut self, st: i32) -> i32 {
        self.oom = st;
        st
    }

    /// Allocate a zeroed node, counted against the node and byte budgets.
    pub fn new_node(&mut self, type_: u32) -> Result<NodeId, i32> {
        if !valid_type(type_) {
            return Err(self.fail(ERR_INTERNAL));
        }
        if self.nodes.len() + 1 > self.max_nodes {
            return Err(self.fail(ERR_LIMIT));
        }
        self.charge(NODE_COST)?;
        self.nodes.mkr_reserve(1).map_err(|_| self.fail(ERR_OOM))?;
        let index = self.nodes.len() as u32;
        let stamp = self.stamp;
        self.nodes.push(Node::zeroed(type_, stamp));
        Ok(NodeId::new(index, stamp))
    }

    /// Expand XML references into one byte-store span.
    pub fn expand(&mut self, src: &[u8], mode: ExpandMode) -> Result<Span, i32> {
        if src.is_empty() {
            return Ok(Span::EMPTY);
        }
        self.charge(src.len())?;
        self.bytes
            .mkr_reserve(src.len())
            .map_err(|_| self.fail(ERR_OOM))?;
        let off = self.bytes.len();
        self.bytes.resize(off + src.len(), 0);
        let n = match expand_into(src, mode, &mut self.bytes[off..]) {
            Ok(n) => n,
            Err(ExpandErr::Syntax) => {
                self.bytes.truncate(off);
                return Err(ERR_SYNTAX);
            }
            Err(ExpandErr::Overflow) => {
                self.bytes.truncate(off);
                return Err(ERR_INTERNAL);
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
    pub fn append_chardata(&mut self, parent: NodeId, type_: u32, span: Span) -> Result<(), i32> {
        if let Some(last) = self.node(parent).last_child {
            if self.node(last).type_ == type_ {
                let old = self.node(last).value;
                if old.end() == span.off as usize {
                    // The two chunks are contiguous in the store (the common
                    // case): extend the span, no copy.
                    let total = (old.len as usize)
                        .checked_add(span.len as usize)
                        .filter(|&t| t <= u32::MAX as usize)
                        .ok_or_else(|| self.fail(ERR_LIMIT))?;
                    self.node_mut(last).value.len = total as u32;
                    return Ok(());
                }
                /* Not contiguous: rebuild the coalesced bytes once. */
                let (a, b) = (old, span);
                let mut merged: Vec<u8> = Vec::new();
                merged
                    .mkr_extend(self.span(a))
                    .and_then(|()| merged.mkr_extend(self.span(b)))
                    .map_err(|_| self.fail(ERR_OOM))?;
                let s = self.store(&merged)?;
                self.node_mut(last).value = s;
                return Ok(());
            }
        }
        let node = self.new_node(type_)?;
        self.node_mut(node).value = span;
        self.append_child(parent, node);
        Ok(())
    }

    /* ---- linking ---- */

    #[inline]
    pub fn set_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.node_mut(id).parent = parent;
    }

    /// Append `child` as the last child of `parent`.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.node_mut(child).parent = Some(parent);
        let last = self.node(parent).last_child;
        if let Some(last) = last {
            self.node_mut(last).next = Some(child);
            self.node_mut(child).prev = Some(last);
        } else {
            self.node_mut(parent).first_child = Some(child);
        }
        self.node_mut(parent).last_child = Some(child);
    }

    /// Unlink `node` from its parent (child chain or attribute chain). No-op
    /// when the node is already detached.
    pub fn detach(&mut self, node: NodeId) {
        let Some(parent) = self.node(node).parent else {
            return;
        };
        if self.node(node).type_ == T_ATTRIBUTE {
            let mut prev: Option<NodeId> = None;
            let mut a = self.node(parent).attrs;
            while let Some(cur) = a {
                if cur == node {
                    self.unlink_attr(parent, prev, cur);
                    break;
                }
                prev = Some(cur);
                a = self.node(cur).next;
            }
            self.clear_links(node);
            return;
        }
        let (prev, next) = (self.node(node).prev, self.node(node).next);
        match prev {
            Some(p) => self.node_mut(p).next = next,
            None => self.node_mut(parent).first_child = next,
        }
        match next {
            Some(n) => self.node_mut(n).prev = prev,
            None => self.node_mut(parent).last_child = prev,
        }
        self.clear_links(node);
    }

    #[inline]
    fn clear_links(&mut self, node: NodeId) {
        let n = self.node_mut(node);
        n.parent = None;
        n.prev = None;
        n.next = None;
    }

    /// Unlink attribute `a` (predecessor `prev`, `None` if head) from `el`.
    pub fn unlink_attr(&mut self, el: NodeId, prev: Option<NodeId>, a: NodeId) {
        let next = self.node(a).next;
        match prev {
            Some(p) => self.node_mut(p).next = next,
            None => self.node_mut(el).attrs = next,
        }
        self.clear_links(a);
    }

    /// Append `attr` to `el`'s attribute list.
    pub fn append_attr(&mut self, el: NodeId, attr: NodeId) {
        self.node_mut(attr).parent = Some(el);
        match self.node(el).attrs {
            None => self.node_mut(el).attrs = Some(attr),
            Some(mut t) => {
                while let Some(n) = self.node(t).next {
                    t = n;
                }
                self.node_mut(t).next = Some(attr);
            }
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
        {
            let n = self.node_mut(node);
            n.parent = Some(container);
            n.prev = prev;
            n.next = next;
        }
        match prev {
            Some(p) => self.node_mut(p).next = Some(node),
            None => self.node_mut(container).first_child = Some(node),
        }
        match next {
            Some(n) => self.node_mut(n).prev = Some(node),
            None => self.node_mut(container).last_child = Some(node),
        }
    }

    /* ---- tree walks ---- */

    /// Pre-order (document-order) successor of `cur` within `root`'s subtree.
    pub fn preorder_next(&self, root: NodeId, cur: NodeId) -> Option<NodeId> {
        if let Some(c) = self.node(cur).first_child {
            return Some(c);
        }
        let mut cur = cur;
        while cur != root && self.node(cur).next.is_none() {
            cur = self.node(cur).parent?;
        }
        if cur == root {
            return None;
        }
        self.node(cur).next
    }

    /// `node`'s topmost ancestor is the document node.
    pub fn is_connected(&self, node: NodeId) -> bool {
        let mut top = node;
        while let Some(p) = self.node(top).parent {
            top = p;
        }
        self.node(top).type_ == T_DOCUMENT
    }

    /// Nearest in-scope binding for `prefix` ("" = default) at or above `node`.
    pub fn resolve_in_scope(&self, node: Option<NodeId>, prefix: &[u8]) -> Option<Span> {
        let mut e = node;
        while let Some(id) = e {
            if self.node(id).type_ == T_ELEMENT {
                let mut a = self.node(id).attrs;
                while let Some(attr) = a {
                    if let Some(p) = crate::xml::qname::xmlns_prefix(self.qname(attr)) {
                        if p == prefix {
                            return Some(self.node(attr).value);
                        }
                    }
                    a = self.node(attr).next;
                }
            }
            e = self.node(id).parent;
        }
        None
    }

    /// True when two attributes share `(local name, namespace URI)`.
    pub fn has_duplicate_attributes(&self, element: NodeId) -> bool {
        let mut a = self.node(element).attrs;
        while let Some(first) = a {
            let mut b = self.node(first).next;
            while let Some(second) = b {
                if self.local(first) == self.local(second) && self.ns(first) == self.ns(second) {
                    return true;
                }
                b = self.node(second).next;
            }
            a = self.node(first).next;
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
        self.has_encoding_decl = 1;
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
        while let Some(cur) = c {
            let t = self.node(cur).type_;
            if self.root.is_none() && t == T_ELEMENT {
                self.root = Some(cur);
            }
            if self.doctype.is_none() && t == T_DOCTYPE {
                self.doctype = Some(cur);
            }
            c = self.node(cur).next;
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

/// Historical free entry points, now thin wrappers over [`Document`]. They keep
/// the FFI adapter's shape while the engine is index-based.
pub fn create_doc(limits: Option<usize>, src_len: usize) -> Result<Box<Document>, i32> {
    Document::create(limits, src_len)
}

/// Turn a raw document handle back into an owned box (the FFI boundary).
///
/// # Safety
/// `doc` must be a pointer returned by [`Document::create`]'s `Box::into_raw`
/// and not yet freed.
pub unsafe fn destroy_doc(doc: *mut Doc) {
    if !doc.is_null() {
        drop(Box::from_raw(doc));
    }
}

#[inline]
pub fn doc_memsize(doc: &Document) -> usize {
    doc.memsize()
}

/// Borrow a byte slice from an FFI `(ptr, len)` pair without copying. Retained
/// for the handful of callers that still receive raw input.
#[inline]
pub fn slice_from_raw<'a>(p: *const c_char, len: u32) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller owns a readable `(p, len)` range for the call.
        unsafe { core::slice::from_raw_parts(p as *const u8, len as usize) }
    }
}

/// A document's `Limits` are read from the raw struct at the FFI boundary.
#[inline]
pub fn limits_max_bytes(limits: &Limits) -> usize {
    limits.max_bytes
}
