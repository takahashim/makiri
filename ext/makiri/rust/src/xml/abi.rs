//! The `mkr_xml_*` C ABI: the node / document layouts, the status and type
//! codes, and the borrowed-slice readers over them.
//!
//! Separate from the engine because the XPath port's XML backend needs these
//! layouts without needing the reader or the mutators - one definition, so the
//! two cannot drift.

/* The readers below all carry one precondition, stated on `bytes`: the (ptr,
 * len) pair names arena bytes that live as long as the document. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_void};

/* ---- status codes (mkr_xml_status_t) ---- */
pub const OK: i32 = 0;
pub const ERR_SYNTAX: i32 = 1;
pub const ERR_LIMIT: i32 = 2;
pub const ERR_OOM: i32 = 3;
pub const ERR_INTERNAL: i32 = 4;
pub const ERR_VERSION: i32 = 5;

/* ---- node types (mkr_xml_node_type_t) ---- */
pub const T_ELEMENT: u32 = 1;
pub const T_ATTRIBUTE: u32 = 2;
pub const T_TEXT: u32 = 3;
pub const T_CDATA: u32 = 4;
pub const T_PI: u32 = 7;
pub const T_COMMENT: u32 = 8;
pub const T_DOCUMENT: u32 = 9;
pub const T_DOCTYPE: u32 = 10;
pub const T_FRAGMENT: u32 = 11;

pub const FLAG_DOM_LOOSE_NAME: u32 = 0x0000_0001;

/// Set on an ELEMENT once its namespace URI has been decided - by the parser,
/// or by resolving it against the context it was first inserted into. From
/// then on the URI is the node's IDENTITY, not a value derived from the
/// declarations around it: moving the node does not change it, and the
/// serializer emits whatever declarations the output needs to reproduce it
/// (the WHATWG DOM model, matching what browsers do). An element still
/// carrying no flag - freshly built by a factory - has no namespace yet and
/// takes one from its insertion context, so building a subtree bottom-up and
/// attaching it gives the same tree as building it top-down.
pub const FLAG_NS_RESOLVED: u32 = 0x0000_0002;

/* ---- mutation status (mkr_xml_mut_status_t) ---- */
pub const MUT_OK: i32 = 0;
pub const MUT_OOM: i32 = 1;
pub const MUT_BAD_NAME: i32 = 2;
pub const MUT_BAD_CHARS: i32 = 3;
pub const MUT_UNBOUND_NS: i32 = 4;
pub const MUT_TYPE: i32 = 5;
pub const MUT_CYCLE: i32 = 6;
pub const MUT_HIERARCHY: i32 = 7;
pub const MUT_BAD_NS_DECL: i32 = 8;

/* ---- budgets (§4) ---- */
pub const MAX_DEPTH: usize = 1024;
pub const MAX_NODES: usize = 10 * 1000 * 1000;
pub const MAX_ATTRS: usize = 4096;
pub const MAX_NS: usize = 4096;
pub const MAX_BYTES: usize = 256 * 1024 * 1024;

pub const XML_NS_URI: &[u8] = b"http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NS_URI: &[u8] = b"http://www.w3.org/2000/xmlns/";

/// The C `""` sentinel: a valid, non-NULL, NUL-terminated empty string that a
/// zero-length slice may point at (never read past, never freed).
pub static EMPTY: [u8; 1] = [0];

#[inline]
pub fn empty() -> *const c_char {
    EMPTY.as_ptr() as *const c_char
}

/// Pointer width, and how far a `u32` field gets padded when the next field is
/// pointer-aligned. The layout asserts below are tripwires for a field added or
/// reordered without the same change in `mkr_xml_node.h`, so they have to hold
/// on every target the gem builds for - including the 32-bit ones.
const PTR: usize = core::mem::size_of::<*const c_char>();
const U32_SLOT: usize = if PTR > 4 { PTR } else { 4 };

/// mkr_xml_node_t - byte-for-byte the C layout.
#[repr(C)]
pub struct Node {
    pub type_: u32,
    pub parent: *mut Node,
    pub first_child: *mut Node,
    pub last_child: *mut Node,
    pub prev: *mut Node,
    pub next: *mut Node,
    pub attrs: *mut Node,
    pub qname: *const c_char,
    pub local: *const c_char,
    pub prefix: *const c_char,
    pub ns_uri: *const c_char,
    pub value: *const c_char,
    pub qname_len: u32,
    pub local_len: u32,
    pub prefix_len: u32,
    pub ns_uri_len: u32,
    pub value_len: u32,
    pub line: u32,
    pub col: u32,
    pub flags: u32,
}
/* 11 pointers, the u32 `type_` in a padded slot, and 8 more u32 */
const _: () = assert!(core::mem::size_of::<Node>() == 11 * PTR + U32_SLOT + 8 * 4);

/// mkr_xml_qname_t.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QName {
    pub qname: *const c_char,
    pub qname_len: u32,
    pub prefix: *const c_char,
    pub prefix_len: u32,
    pub local: *const c_char,
    pub local_len: u32,
}

/// An arena chunk header; the payload follows it, aligned. Part of the document
/// layout because `Doc.chunks` points at one - the arena owns the allocation.
#[repr(C)]
pub struct Chunk {
    pub(crate) next: *mut Chunk,
    pub(crate) used: usize,
    pub(crate) cap: usize,
}

/// mkr_xml_doc_t.
#[repr(C)]
pub struct Doc {
    pub chunks: *mut Chunk,
    pub arena_bytes: usize,
    pub max_bytes: usize,
    pub nodes: usize,
    pub max_nodes: usize,
    pub oom: i32,
    pub root: *mut Node,
    pub doc_node: *mut Node,
    pub doctype: *mut Node,
    pub name_index: *mut c_void,
    pub has_encoding_decl: i32,
}
/* 9 pointer-sized fields (5 pointers + 4 usize) and 2 i32, each padded */
const _: () = assert!(core::mem::size_of::<Doc>() == 9 * PTR + 2 * U32_SLOT);

/// mkr_xml_limits_t.
#[repr(C)]
pub struct Limits {
    pub max_bytes: usize,
}

/// mkr_spanbuf_t (core/mkr_buf.h) - returned BY VALUE by mkr_xml_arena_spanbuf.
#[repr(C)]
pub struct SpanBuf {
    pub buf: *mut c_char,
    pub cap: usize,
    pub pos: usize,
    pub ok: bool,
}

/// View a C (ptr,len) pair as a byte slice. NULL or len 0 is the empty slice,
/// so a "" / NULL field never gets dereferenced. The lifetime is the caller's
/// claim (arena-owned bytes live as long as the document).
#[inline]
pub unsafe fn bytes<'a>(p: *const c_char, len: u32) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p as *const u8, len as usize)
    }
}

#[inline]
pub unsafe fn node_qname<'a>(n: *const Node) -> &'a [u8] {
    bytes((*n).qname, (*n).qname_len)
}
#[inline]
pub unsafe fn node_local<'a>(n: *const Node) -> &'a [u8] {
    bytes((*n).local, (*n).local_len)
}
#[inline]
pub unsafe fn node_prefix<'a>(n: *const Node) -> &'a [u8] {
    bytes((*n).prefix, (*n).prefix_len)
}
#[inline]
pub unsafe fn node_ns<'a>(n: *const Node) -> &'a [u8] {
    bytes((*n).ns_uri, (*n).ns_uri_len)
}
#[inline]
pub unsafe fn node_value<'a>(n: *const Node) -> &'a [u8] {
    bytes((*n).value, (*n).value_len)
}

/// The QName parts a built node carries (mkr_xml_qname_of).
#[inline]
pub unsafe fn qname_of(n: *const Node) -> QName {
    QName {
        qname: (*n).qname,
        qname_len: (*n).qname_len,
        prefix: (*n).prefix,
        prefix_len: (*n).prefix_len,
        local: (*n).local,
        local_len: (*n).local_len,
    }
}

