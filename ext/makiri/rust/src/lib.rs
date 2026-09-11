//! makiri_xml - the Makiri XML reader / arena / mutators, ported from
//! ext/makiri/xml/*.c behind the SAME C ABI (symbol names, struct layouts,
//! status codes), so glue/, dom_adapter/ and the XPath XML backend link against
//! it unchanged.
//!
//! Layering (the point of the spike is to measure how much of the engine can
//! be safe code when the tree is a C-layout, pointer-linked arena):
//!
//!   chars.rs   pure byte/codepoint primitives + reference expansion  (no unsafe)
//!   qname.rs   QName splitting / xmlns detection                      (no unsafe)
//!   arena.rs   the append-only arena and node allocation              (unsafe: raw memory)
//!   tree.rs    tokenizer + tree builder                               (scanning is safe;
//!                                                                     node linking unsafe)
//!   mutate.rs  mutation primitives                                   (unsafe: walks raw nodes)
//!   index.rs   element-name index                                    (unsafe: walks raw nodes)
//!   ffi.rs     the exported `mkr_xml_*` symbols                       (unsafe boundary)
//!   selftest.rs the three C self-tests, ported                       (test code)

#![allow(clippy::missing_safety_doc)]

pub mod arena;
pub mod chars;
pub mod ffi;
pub mod index;
pub mod mutate;
pub mod qname;
pub mod selftest;
pub mod tree;

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

/// mkr_xml_node_t - byte-for-byte the C layout (128 bytes).
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
const _: () = assert!(core::mem::size_of::<Node>() == 128);

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

/// mkr_xml_doc_t.
#[repr(C)]
pub struct Doc {
    pub chunks: *mut arena::Chunk,
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
const _: () = assert!(core::mem::size_of::<Doc>() == 88);

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

/// A QName value over `name` split per `sp` (prefix at offset 0, local at
/// `local_off`) - the layout the tree builder and mutators hand to
/// qname_assign. Safe: pointer arithmetic stays within `name`.
#[inline]
pub fn qname_from(name: &[u8], sp: &qname::Split) -> QName {
    QName {
        qname: name.as_ptr() as *const c_char,
        qname_len: name.len() as u32,
        prefix: name.as_ptr() as *const c_char,
        prefix_len: sp.prefix_len,
        local: name[sp.local_off as usize..].as_ptr() as *const c_char,
        local_len: sp.local_len,
    }
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
