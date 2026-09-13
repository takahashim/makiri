//! The secure-by-design append-only arena (mkr_xml_node.c). The one
//! overflow- and budget-checked allocation choke point is `arena_alloc`;
//! callers get typed nodes or copied bytes, never a raw cut to cast.
//!
//! This module is inherently unsafe: it hands out raw memory that the C side
//! reads through `mkr_xml_node_t` field access.

/* One precondition throughout: `doc` is a live document, and any node handed
 * in was allocated from its arena. The module header says why that is the
 * boundary. */
#![allow(clippy::missing_safety_doc)]

use crate::xml::chars::{expand_into, ExpandErr, ExpandMode};
use crate::xml::{
    bytes, empty, index, Chunk, Doc, Node, QName, SpanBuf, ERR_INTERNAL, ERR_LIMIT, ERR_OOM,
    ERR_SYNTAX, MAX_BYTES, MAX_NODES, T_ATTRIBUTE, T_CDATA, T_COMMENT, T_DOCTYPE, T_DOCUMENT,
    T_ELEMENT, T_FRAGMENT, T_PI, T_TEXT,
};
use core::ffi::c_char;
use core::ptr::{self, NonNull};
use std::alloc::{alloc, dealloc, Layout};

/// Alignment of every cut: the strictest fundamental alignment (max_align_t).
const ALIGN: usize = 16;
const CHUNK_MIN: usize = 64 * 1024;

/// The parser's exclusive handle to one live XML arena.  It deliberately owns
/// no memory: `Doc` continues to own the arena, while this type keeps parser
/// code from repeatedly opening raw `Doc*` dereferences.
#[derive(Clone, Copy)]
pub(crate) struct ParserArena {
    doc: NonNull<Doc>,
}

impl ParserArena {
    #[inline]
    pub(crate) fn new(doc: NonNull<Doc>) -> Self {
        Self { doc }
    }

    #[inline]
    pub(crate) fn as_ptr(self) -> *mut Doc {
        self.doc.as_ptr()
    }

    #[inline]
    pub(crate) fn status(self) -> i32 {
        // SAFETY: ParserArena is created only for a live document and parsing
        // has exclusive access under the Ruby GVL.
        unsafe { self.doc.as_ref().oom }
    }

    #[inline]
    pub(crate) fn document_node(self) -> *mut Node {
        // SAFETY: see `status`.
        unsafe { self.doc.as_ref().doc_node }
    }

    #[inline]
    pub(crate) fn root(self) -> *mut Node {
        // SAFETY: see `status`.
        unsafe { self.doc.as_ref().root }
    }

    #[inline]
    pub(crate) fn set_root(self, node: *mut Node) {
        // SAFETY: parser-only initialization of this live document.
        unsafe { (*self.doc.as_ptr()).root = node }
    }

    #[inline]
    pub(crate) fn set_doctype(self, node: *mut Node) {
        // SAFETY: parser-only initialization of this live document.
        unsafe { (*self.doc.as_ptr()).doctype = node }
    }

    #[inline]
    pub(crate) fn mark_encoding_decl(self) {
        // SAFETY: parser-only initialization of this live document.
        unsafe { (*self.doc.as_ptr()).has_encoding_decl = 1 }
    }

    #[inline]
    pub(crate) fn bytes(self, src: &[u8]) -> *const c_char {
        // SAFETY: ParserArena guarantees a live arena for the allocation.
        unsafe { arena_bytes(self.as_ptr(), src) }
    }

    #[inline]
    pub(crate) fn node(self, type_: u32) -> *mut Node {
        // SAFETY: ParserArena guarantees a live arena for the allocation.
        unsafe { arena_node(self.as_ptr(), type_) }
    }

    #[inline]
    pub(crate) fn assign_qname(self, node: *mut Node, qn: &QName) -> i32 {
        // SAFETY: `node` was allocated from this arena immediately before use.
        unsafe { qname_assign(self.as_ptr(), node, qn) }
    }

    #[inline]
    pub(crate) fn append(self, parent: *mut Node, child: *mut Node) {
        // SAFETY: parser builds a tree from fresh nodes in one arena.
        unsafe { append_child(parent, child) }
    }

    #[inline]
    pub(crate) fn expand(self, src: &[u8], mode: ExpandMode) -> Result<(*const c_char, u32), i32> {
        // SAFETY: ParserArena guarantees a live document for the arena cut.
        unsafe { expand_arena(self.as_ptr(), src, mode) }
    }

    #[inline]
    pub(crate) fn append_chardata(
        self,
        parent: *mut Node,
        type_: u32,
        value: *const c_char,
        len: u32,
    ) -> Result<(), i32> {
        // SAFETY: the parser passes a parent and value from this live arena.
        unsafe { append_chardata(self.as_ptr(), parent, type_, value, len) }
    }
}

/// Where a chunk's payload starts: the header size rounded up to ALIGN.
const HDR: usize = (core::mem::size_of::<Chunk>() + ALIGN - 1) & !(ALIGN - 1);

#[inline]
fn align_up(n: usize) -> Option<usize> {
    n.checked_add(ALIGN - 1).map(|x| x & !(ALIGN - 1))
}

pub unsafe fn doc_new() -> *mut Doc {
    // Null on failure: every caller already treats a null document as the OOM
    // answer, because the chunk allocator below can return one too.
    crate::falloc::try_box_raw(Doc {
        chunks: ptr::null_mut(),
        arena_bytes: 0,
        max_bytes: MAX_BYTES,
        nodes: 0,
        max_nodes: MAX_NODES,
        oom: 0,
        root: ptr::null_mut(),
        doc_node: ptr::null_mut(),
        doctype: ptr::null_mut(),
        name_index: None,
        has_encoding_decl: 0,
    })
}

/// Whole-arena free: no individual node / byte free anywhere.
pub unsafe fn doc_destroy(doc: *mut Doc) {
    if doc.is_null() {
        return;
    }
    index::invalidate(&mut *doc);
    let mut c = (*doc).chunks;
    while !c.is_null() {
        let n = (*c).next;
        let cap = (*c).cap;
        /* HDR + cap was checked to fit when the chunk was allocated */
        let layout = Layout::from_size_align_unchecked(HDR + cap, ALIGN);
        dealloc(c as *mut u8, layout);
        c = n;
    }
    drop(Box::from_raw(doc));
}

pub unsafe fn doc_memsize(doc: *const Doc) -> usize {
    if doc.is_null() {
        return 0;
    }
    let mut total = core::mem::size_of::<Doc>();
    let mut c = (*doc).chunks;
    while !c.is_null() {
        /* saturate rather than wrap: a bogus huge memsize is harmless */
        match HDR
            .checked_add((*c).cap)
            .and_then(|chunk| total.checked_add(chunk))
        {
            Some(t) => total = t,
            None => return usize::MAX,
        }
        c = (*c).next;
    }
    total
}

/// THE single checked alloc choke point. On any failure sets doc.oom (sticky)
/// and returns null. Nothing else cuts arena.
pub unsafe fn arena_alloc(doc: *mut Doc, size: usize) -> *mut u8 {
    if doc.is_null() || (*doc).oom != 0 {
        return ptr::null_mut();
    }
    let need = match align_up(size) {
        Some(n) => n,
        None => {
            (*doc).oom = ERR_LIMIT;
            return ptr::null_mut();
        }
    };
    /* budget BEFORE allocation - fail-closed */
    match (*doc).arena_bytes.checked_add(need) {
        Some(p) if p <= (*doc).max_bytes => {}
        _ => {
            (*doc).oom = ERR_LIMIT;
            return ptr::null_mut();
        }
    }
    let mut c = (*doc).chunks;
    if c.is_null() || need > (*c).cap - (*c).used {
        let cap = if need > CHUNK_MIN { need } else { CHUNK_MIN };
        let total = match HDR.checked_add(cap) {
            Some(t) => t,
            None => {
                (*doc).oom = ERR_LIMIT;
                return ptr::null_mut();
            }
        };
        let layout = match Layout::from_size_align(total, ALIGN) {
            Ok(l) => l,
            Err(_) => {
                (*doc).oom = ERR_LIMIT;
                return ptr::null_mut();
            }
        };
        // The arena's one libc allocation, so the sweep's consult belongs
        // here. The branch below already handles a null, which is what makes
        // this the cheapest place in the crate to be injectable.
        let nc = if crate::falloc::should_fail() {
            ptr::null_mut()
        } else {
            alloc(layout) as *mut Chunk
        };
        if nc.is_null() {
            (*doc).oom = ERR_OOM;
            return ptr::null_mut();
        }
        (*nc).next = (*doc).chunks;
        (*nc).used = 0;
        (*nc).cap = cap;
        (*doc).chunks = nc;
        c = nc;
    }
    let p = (c as *mut u8).add(HDR + (*c).used);
    (*c).used += need;
    (*doc).arena_bytes += need;
    p
}

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

/// A zeroed node of `type_`, counted against the node budget.
pub unsafe fn arena_node(doc: *mut Doc, type_: u32) -> *mut Node {
    if doc.is_null() {
        return ptr::null_mut();
    }
    if !valid_type(type_) {
        (*doc).oom = ERR_INTERNAL;
        return ptr::null_mut();
    }
    if (*doc).nodes + 1 > (*doc).max_nodes {
        (*doc).oom = ERR_LIMIT;
        return ptr::null_mut();
    }
    let n = arena_alloc(doc, core::mem::size_of::<Node>()) as *mut Node;
    if n.is_null() {
        return n;
    }
    ptr::write_bytes(n as *mut u8, 0, core::mem::size_of::<Node>());
    (*n).type_ = type_;
    (*doc).nodes += 1;
    n
}

/// Copy `src` into the arena (copy-on-store). len 0 -> the "" sentinel.
pub unsafe fn arena_bytes(doc: *mut Doc, src: &[u8]) -> *const c_char {
    if src.is_empty() {
        return empty();
    }
    let p = arena_alloc(doc, src.len());
    if p.is_null() {
        return ptr::null();
    }
    ptr::copy_nonoverlapping(src.as_ptr(), p, src.len());
    p as *const c_char
}

/// Expand XML references into one arena cut. The raw document pointer stays
/// inside the arena layer; parser code sees only the resulting byte slice.
pub unsafe fn expand_arena(
    doc: *mut Doc,
    src: &[u8],
    mode: ExpandMode,
) -> Result<(*const c_char, u32), i32> {
    if src.is_empty() {
        return Ok((empty(), 0));
    }
    let out = match arena_cut(doc, src.len()) {
        Some(out) => out,
        None => return Err((*doc).oom),
    };
    let base = out.as_ptr() as *const c_char;
    match expand_into(src, mode, out) {
        Ok(n) => Ok((base, n as u32)),
        Err(ExpandErr::Syntax) => Err(ERR_SYNTAX),
        Err(ExpandErr::Overflow) => Err(ERR_INTERNAL),
    }
}

/// Append character data, coalescing an adjacent node of the same type.
/// Allocation and node-field mutation are kept together so a failed cut can
/// never leave the linked tree half-updated.
unsafe fn append_chardata(
    doc: *mut Doc,
    parent: *mut Node,
    type_: u32,
    value: *const c_char,
    len: u32,
) -> Result<(), i32> {
    let last = (*parent).last_child;
    if !last.is_null() && (*last).type_ == type_ {
        let total = match ((*last).value_len as usize).checked_add(len as usize) {
            Some(total) if total <= u32::MAX as usize => total,
            _ => return Err(ERR_LIMIT),
        };
        if total == 0 {
            (*last).value = empty();
            (*last).value_len = 0;
            return Ok(());
        }
        let buf = match arena_cut(doc, total) {
            Some(buf) => buf,
            None => return Err((*doc).oom),
        };
        let old = bytes((*last).value, (*last).value_len);
        buf[..old.len()].copy_from_slice(old);
        buf[old.len()..].copy_from_slice(bytes(value, len));
        (*last).value = buf.as_ptr() as *const c_char;
        (*last).value_len = total as u32;
        return Ok(());
    }

    let node = arena_node(doc, type_);
    if node.is_null() {
        return Err((*doc).oom);
    }
    (*node).value = value;
    (*node).value_len = len;
    append_child(parent, node);
    Ok(())
}

/// A raw arena cut of `len` bytes as a mutable slice for the caller to fill
/// (the Rust-side counterpart of mkr_xml_arena_spanbuf). None on failure
/// (doc.oom set). len 0 yields an empty slice at the "" sentinel.
pub unsafe fn arena_cut<'a>(doc: *mut Doc, len: usize) -> Option<&'a mut [u8]> {
    if len == 0 {
        return Some(&mut []);
    }
    let p = arena_alloc(doc, len);
    if p.is_null() {
        None
    } else {
        Some(core::slice::from_raw_parts_mut(p, len))
    }
}

/// mkr_xml_arena_spanbuf: carve `cap` bytes and wrap them in the C bounded
/// writer. On alloc failure the writer is already not-ok (buf == NULL).
pub unsafe fn arena_spanbuf(doc: *mut Doc, cap: usize) -> SpanBuf {
    let buf: *mut u8 = if cap == 0 {
        empty() as *mut u8
    } else {
        arena_alloc(doc, cap)
    };
    SpanBuf {
        buf: buf as *mut c_char,
        cap,
        pos: 0,
        ok: !buf.is_null(),
    }
}

/// Copy `qn`'s name into the arena as one contiguous slice and point the
/// node's qname/local/prefix into it. 0 on success, -1 on arena OOM (node left
/// untouched) or a contract violation (local not aliasing into qname -> doc.oom
/// = INTERNAL). mkr_xml_qname_assign.
pub unsafe fn qname_assign(doc: *mut Doc, node: *mut Node, qn: &QName) -> i32 {
    let q0 = qn.qname as usize;
    let l0 = qn.local as usize;
    let aliases = l0 >= q0
        && qn.local_len <= qn.qname_len
        && (l0 - q0) <= (qn.qname_len - qn.local_len) as usize;
    if !aliases {
        if !doc.is_null() {
            (*doc).oom = ERR_INTERNAL;
        }
        return -1;
    }
    let q = arena_bytes(doc, bytes(qn.qname, qn.qname_len));
    if qn.qname_len > 0 && q.is_null() {
        return -1;
    }
    (*node).qname = q;
    (*node).qname_len = qn.qname_len;
    (*node).local = (q as *const u8).add(l0 - q0) as *const c_char;
    (*node).local_len = qn.local_len;
    (*node).prefix = q;
    (*node).prefix_len = qn.prefix_len;
    0
}

/// Pre-order (document-order) successor of `cur` within `root`'s subtree, or
/// null once the subtree is exhausted. mkr_xml_preorder_next.
pub unsafe fn preorder_next(root: *const Node, mut cur: *mut Node) -> *mut Node {
    if !(*cur).first_child.is_null() {
        return (*cur).first_child;
    }
    while !ptr::eq(cur, root) && (*cur).next.is_null() {
        cur = (*cur).parent;
    }
    if ptr::eq(cur, root) {
        return ptr::null_mut();
    }
    (*cur).next
}

/// Append `child` as the last child of `parent` (the parser's link helper).
#[inline]
pub unsafe fn append_child(parent: *mut Node, child: *mut Node) {
    (*child).parent = parent;
    let last = (*parent).last_child;
    if !last.is_null() {
        (*last).next = child;
        (*child).prev = last;
    } else {
        (*parent).first_child = child;
    }
    (*parent).last_child = child;
}
