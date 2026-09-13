//! Minimal raw node access and pointer linking for the XML arena.
//!
//! `NodeRef` is the boundary the mutation layer composes: it carries the
//! "non-NULL, arena-owned node" invariant, while every dereference of the
//! C-layout `mkr_xml_node_t` lives here. `mutate.rs` can then be ordinary Rust
//! under `#![forbid(unsafe_code)]`; the one unsafe constructor,
//! [`NodeRef::from_raw`], is called only by `ffi.rs` and the cross-kind
//! importer when they turn a raw pointer into a reference.

/* One precondition throughout: any node handed in was allocated from a live
 * document arena that outlives every use of the reference. `NodeRef::from_raw`
 * states it at the one place it can be established. Construction is limited
 * to raw-pointer boundaries (`ffi.rs`, fresh arena allocations, cross-kind
 * import, and self-tests); mutation code only passes an existing `NodeRef`. */

use crate::xml::qname::xmlns_prefix;
use crate::xml::{
    bytes as abi_bytes, node_local, node_ns, node_qname, node_value, qname_of, Doc, Node, QName,
    T_ATTRIBUTE, T_DOCUMENT, T_ELEMENT,
};
use core::ffi::c_char;
use core::ptr::{self, NonNull};

/// A non-NULL, arena-owned XML node.
///
/// The type carries "not NULL"; that the node belongs to a live document arena
/// and is not aliased for mutation is the caller's contract, established at
/// [`NodeRef::from_raw`] and maintained by mutation serialising under the Ruby
/// GVL.
///
/// `repr(transparent)`: a `NodeRef` slice has the layout of a `*mut Node`
/// slice, so the name-index bucket can be handed to the engine as one.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct NodeRef(NonNull<Node>);

impl PartialEq for NodeRef {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for NodeRef {}

impl NodeRef {
    /// # Safety
    /// `p` must be null, or point to a node in an arena that stays live and
    /// un-aliased for mutation for as long as the returned value is used.
    #[inline]
    pub(crate) unsafe fn from_raw(p: *mut Node) -> Option<Self> {
        NonNull::new(p).map(NodeRef)
    }

    #[inline]
    pub(crate) fn as_ptr(self) -> *mut Node {
        self.0.as_ptr()
    }

    /* ---- scalar reads ---- */

    #[inline]
    pub(crate) fn type_(self) -> u32 {
        unsafe { (*self.0.as_ptr()).type_ }
    }
    #[inline]
    pub(crate) fn flags(self) -> u32 {
        unsafe { (*self.0.as_ptr()).flags }
    }
    #[inline]
    pub(crate) fn qname_len(self) -> u32 {
        unsafe { (*self.0.as_ptr()).qname_len }
    }
    #[inline]
    pub(crate) fn local_len(self) -> u32 {
        unsafe { (*self.0.as_ptr()).local_len }
    }
    #[inline]
    pub(crate) fn ns_uri_len(self) -> u32 {
        unsafe { (*self.0.as_ptr()).ns_uri_len }
    }
    #[inline]
    pub(crate) fn value_len(self) -> u32 {
        unsafe { (*self.0.as_ptr()).value_len }
    }

    /* ---- borrowed field reads (arena lifetime) ---- */

    #[inline]
    pub(crate) fn qname<'a>(self) -> &'a [u8] {
        unsafe { node_qname(self.0.as_ptr()) }
    }
    #[inline]
    pub(crate) fn local<'a>(self) -> &'a [u8] {
        unsafe { node_local(self.0.as_ptr()) }
    }
    #[inline]
    pub(crate) fn ns<'a>(self) -> &'a [u8] {
        unsafe { node_ns(self.0.as_ptr()) }
    }
    #[inline]
    pub(crate) fn value<'a>(self) -> &'a [u8] {
        unsafe { node_value(self.0.as_ptr()) }
    }
    #[inline]
    pub(crate) fn qname_of(self) -> QName {
        unsafe { qname_of(self.0.as_ptr()) }
    }
    #[inline]
    pub(crate) fn qname_ptr(self) -> *const c_char {
        unsafe { (*self.0.as_ptr()).qname }
    }
    #[inline]
    pub(crate) fn value_ptr(self) -> *const c_char {
        unsafe { (*self.0.as_ptr()).value }
    }
    #[inline]
    pub(crate) fn ns_uri_ptr(self) -> *const c_char {
        unsafe { (*self.0.as_ptr()).ns_uri }
    }

    /* ---- link reads ---- */

    #[inline]
    pub(crate) fn parent(self) -> Option<Self> {
        unsafe { Self::from_raw((*self.0.as_ptr()).parent) }
    }
    #[inline]
    pub(crate) fn first_child(self) -> Option<Self> {
        unsafe { Self::from_raw((*self.0.as_ptr()).first_child) }
    }
    #[inline]
    pub(crate) fn last_child(self) -> Option<Self> {
        unsafe { Self::from_raw((*self.0.as_ptr()).last_child) }
    }
    #[inline]
    pub(crate) fn prev(self) -> Option<Self> {
        unsafe { Self::from_raw((*self.0.as_ptr()).prev) }
    }
    #[inline]
    pub(crate) fn next(self) -> Option<Self> {
        unsafe { Self::from_raw((*self.0.as_ptr()).next) }
    }
    #[inline]
    pub(crate) fn attrs(self) -> Option<Self> {
        unsafe { Self::from_raw((*self.0.as_ptr()).attrs) }
    }

    /* ---- field writes ---- */

    #[inline]
    pub(crate) fn set_ns(self, uri: *const c_char, len: u32) {
        unsafe {
            (*self.0.as_ptr()).ns_uri = uri;
            (*self.0.as_ptr()).ns_uri_len = len;
        }
    }
    #[inline]
    pub(crate) fn set_value(self, value: *const c_char, len: u32) {
        unsafe {
            (*self.0.as_ptr()).value = value;
            (*self.0.as_ptr()).value_len = len;
        }
    }
    #[inline]
    pub(crate) fn set_local(self, value: *const c_char, len: u32) {
        unsafe {
            (*self.0.as_ptr()).local = value;
            (*self.0.as_ptr()).local_len = len;
        }
    }
    #[inline]
    pub(crate) fn set_qname_parts(self, value: *const c_char, len: u32) {
        unsafe {
            (*self.0.as_ptr()).qname = value;
            (*self.0.as_ptr()).qname_len = len;
        }
    }
    #[inline]
    pub(crate) fn set_prefix(self, value: *const c_char, len: u32) {
        unsafe {
            (*self.0.as_ptr()).prefix = value;
            (*self.0.as_ptr()).prefix_len = len;
        }
    }
    #[inline]
    pub(crate) fn set_flags(self, flags: u32) {
        unsafe { (*self.0.as_ptr()).flags = flags }
    }
    #[inline]
    pub(crate) fn add_flag(self, flag: u32) {
        unsafe { (*self.0.as_ptr()).flags |= flag }
    }
    #[inline]
    pub(crate) fn clear_flag(self, flag: u32) {
        unsafe { (*self.0.as_ptr()).flags &= !flag }
    }
    #[inline]
    pub(crate) fn set_parent(self, parent: Option<Self>) {
        unsafe { (*self.0.as_ptr()).parent = ptr_of(parent) }
    }
    #[inline]
    pub(crate) fn set_prev(self, prev: Option<Self>) {
        unsafe { (*self.0.as_ptr()).prev = ptr_of(prev) }
    }
    #[inline]
    pub(crate) fn set_next(self, next: Option<Self>) {
        unsafe { (*self.0.as_ptr()).next = ptr_of(next) }
    }
    #[inline]
    pub(crate) fn set_first_child(self, first: Option<Self>) {
        unsafe { (*self.0.as_ptr()).first_child = ptr_of(first) }
    }
    #[inline]
    pub(crate) fn set_last_child(self, last: Option<Self>) {
        unsafe { (*self.0.as_ptr()).last_child = ptr_of(last) }
    }
    #[inline]
    pub(crate) fn set_attrs(self, attrs: Option<Self>) {
        unsafe { (*self.0.as_ptr()).attrs = ptr_of(attrs) }
    }

    /// Drop every structural link (parent / prev / next), as `detach` leaves a
    /// removed node.
    #[inline]
    pub(crate) fn clear_links(self) {
        self.set_parent(None);
        self.set_prev(None);
        self.set_next(None);
    }
}

#[inline]
fn ptr_of(node: Option<NodeRef>) -> *mut Node {
    match node {
        Some(n) => n.as_ptr(),
        None => ptr::null_mut(),
    }
}

/* ================================ primitives ============================== */

/// View an arena `(ptr, len)` pair as a byte slice (the safe face of
/// `abi::bytes`, whose lifetime is the arena's).
#[inline]
pub(crate) fn bytes<'a>(p: *const c_char, len: u32) -> &'a [u8] {
    unsafe { abi_bytes(p, len) }
}

/// Nearest in-scope binding for `prefix` ("" = default) at or above `node`,
/// or `None` when nothing binds it.
pub(crate) fn resolve_in_scope(
    node: Option<NodeRef>,
    prefix: &[u8],
) -> Option<(*const c_char, u32)> {
    let mut e = node;
    while let Some(n) = e {
        if n.type_() == T_ELEMENT {
            let mut a = n.attrs();
            while let Some(attr) = a {
                if let Some(p) = xmlns_prefix(attr.qname()) {
                    if p == prefix {
                        return Some((attr.value_ptr(), attr.value_len()));
                    }
                }
                a = attr.next();
            }
        }
        e = n.parent();
    }
    None
}

/// The document node of a live document, as a non-null reference.
#[inline]
pub(crate) fn document_node(doc: &Doc) -> Option<NodeRef> {
    // SAFETY: `doc` is live and `doc_node` is a node in its arena.
    unsafe { NodeRef::from_raw(doc.doc_node) }
}

/// `node`'s topmost ancestor is the document node.
pub(crate) fn is_connected(node: NodeRef) -> bool {
    let mut top = node;
    while let Some(p) = top.parent() {
        top = p;
    }
    top.type_() == T_DOCUMENT
}

/// Pre-order (document-order) successor of `cur` within `root`'s subtree.
pub(crate) fn preorder_next(root: NodeRef, cur: NodeRef) -> Option<NodeRef> {
    unsafe {
        NodeRef::from_raw(crate::xml::arena::preorder_next(
            root.as_ptr(),
            cur.as_ptr(),
        ))
    }
}

/// Append `child` as the last child of `parent` (parser / clone link helper).
#[inline]
pub(crate) fn append_child(parent: NodeRef, child: NodeRef) {
    unsafe { crate::xml::arena::append_child(parent.as_ptr(), child.as_ptr()) }
}

/// Unlink attribute `a` (predecessor `prev`, `None` if head) from `el`.
pub(crate) fn unlink_attr(el: NodeRef, prev: Option<NodeRef>, a: NodeRef) {
    match prev {
        Some(p) => p.set_next(a.next()),
        None => el.set_attrs(a.next()),
    }
    a.clear_links();
}

/// Append `attr` to `el`'s attribute list (walking to the tail).
pub(crate) fn append_attr(el: NodeRef, attr: NodeRef) {
    attr.set_parent(Some(el));
    match el.attrs() {
        None => el.set_attrs(Some(attr)),
        Some(mut t) => {
            while let Some(n) = t.next() {
                t = n;
            }
            t.set_next(Some(attr));
        }
    }
}

/// Unlink `node` from its parent (child chain or attribute chain). No-op when
/// the node is already detached.
pub(crate) fn detach(node: NodeRef) {
    let Some(parent) = node.parent() else {
        return;
    };
    if node.type_() == T_ATTRIBUTE {
        let mut prev: Option<NodeRef> = None;
        let mut a = parent.attrs();
        while let Some(cur) = a {
            if cur == node {
                unlink_attr(parent, prev, cur);
                break;
            }
            prev = Some(cur);
            a = cur.next();
        }
        node.clear_links();
        return;
    }
    match node.prev() {
        Some(p) => p.set_next(node.next()),
        None => parent.set_first_child(node.next()),
    }
    match node.next() {
        Some(n) => n.set_prev(node.prev()),
        None => parent.set_last_child(node.prev()),
    }
    node.clear_links();
}

/// The ONE place the doubly-linked child list is written by insertion.
pub(crate) fn splice_between(
    container: NodeRef,
    node: NodeRef,
    prev: Option<NodeRef>,
    next: Option<NodeRef>,
) {
    node.set_parent(Some(container));
    node.set_prev(prev);
    node.set_next(next);
    match prev {
        Some(p) => p.set_next(Some(node)),
        None => container.set_first_child(Some(node)),
    }
    match next {
        Some(n) => n.set_prev(Some(node)),
        None => container.set_last_child(Some(node)),
    }
}
