//! The XML node model: the status and type codes, the index-based node and
//! document layouts, and the handle types shared with the XPath backend and the
//! Ruby glue.
//!
//! The tree is an **index arena**: a [`Document`] owns a `Vec<Node>` and a byte
//! store, a node reference is a [`NodeId`] (index plus generation), structural
//! links are `Option<NodeId>`, and every name/value is an `(offset, len)` into
//! the document's byte store. No raw pointer is part of the model, so the
//! parser, mutators, index and XPath XML backend can all be ordinary safe Rust.

/* Boundary readers state their precondition once, on `bytes`. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_char;

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

/// A byte span into a [`Document`]'s byte store. `off` 0 with `len` 0 is the
/// always-valid empty slice; no span ever points outside the store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub off: u32,
    pub len: u32,
}

impl Span {
    /// A zero-length span at offset 0 (the empty string).
    pub(crate) const EMPTY: Span = Span { off: 0, len: 0 };
    /// The "field never set" marker. Distinct from an empty literal, which the
    /// doctype's PUBLIC/SYSTEM identifiers need (`PUBLIC ""` is present, not
    /// absent). Reading an `ABSENT` span yields the empty slice.
    pub(crate) const ABSENT: Span = Span {
        off: u32::MAX,
        len: 0,
    };
    #[inline]
    pub(crate) fn is_absent(self) -> bool {
        self.off == u32::MAX
    }
    #[inline]
    pub(crate) fn end(self) -> usize {
        self.off as usize + self.len as usize
    }
}

/// A handle to one node in a live [`Document`], packed into one word: the low
/// 32 bits are the slot index, the high 32 the generation.
///
/// The one-word form is deliberate. The engine carries nodes through node-sets
/// as opaque tokens it only compares and hashes, so a node id *is* that token:
/// an `&[NodeId]` is layout-identical to the engine's `*mut c_void` buffer, and
/// `to_token`/`from_token` cost nothing. That packing needs a 64-bit word, which
/// the `compile_error!` below enforces.
///
/// `generation` is a stamp that identifies the OWNING document. Because slots
/// are never recycled (detach never destroys, so a removed node stays
/// addressable for live Ruby wrappers), there is no reuse tag to carry; using
/// the field as a document stamp instead lets [`Document::try_node`] reject a
/// handle built for another document. If slots are ever recycled it becomes
/// index + document/reuse stamp, and [`Document::try_node`] already checks it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(usize);

#[cfg(not(target_pointer_width = "64"))]
compile_error!(
    "Makiri's XML index arena packs a NodeId into one pointer-sized word; \
     only 64-bit targets are supported"
);

impl NodeId {
    /// The absent handle. It packs to a NULL `*mut c_void` token, which is the
    /// engine's "no node", so index 0 is reserved and never a real node.
    pub const INVALID: NodeId = NodeId(0);

    #[inline]
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        NodeId(((generation as usize) << 32) | index as usize)
    }
    #[inline]
    pub fn index(self) -> u32 {
        self.0 as u32
    }
    #[inline]
    pub fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
    #[inline]
    pub fn is_invalid(self) -> bool {
        self.index() == 0
    }

    /// The opaque token the engine carries in node-sets (identity here).
    #[inline]
    pub(crate) fn to_token(self) -> usize {
        self.0
    }
    #[inline]
    pub(crate) fn from_token(token: usize) -> Self {
        NodeId(token)
    }
}

/// One node in the document's slot array.
///
/// Links are `Option<NodeId>` and byte fields are spans, so a `Node` is plain
/// data with no pointer to chase.
pub struct Node {
    pub type_: u32,
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub last_child: Option<NodeId>,
    pub prev: Option<NodeId>,
    pub next: Option<NodeId>,
    pub attrs: Option<NodeId>,
    pub qname: Span,
    pub local: Span,
    pub prefix: Span,
    pub ns_uri: Span,
    pub value: Span,
    pub line: u32,
    pub col: u32,
    pub flags: u32,
    pub(crate) generation: u32,
}

impl Node {
    pub(crate) fn zeroed(type_: u32, generation: u32) -> Self {
        Node {
            type_,
            parent: None,
            first_child: None,
            last_child: None,
            prev: None,
            next: None,
            attrs: None,
            qname: Span::ABSENT,
            local: Span::ABSENT,
            prefix: Span::ABSENT,
            ns_uri: Span::ABSENT,
            value: Span::ABSENT,
            line: 0,
            col: 0,
            flags: 0,
            generation,
        }
    }
}

/// The per-document allocation limit.
pub struct Limits {
    pub max_bytes: usize,
}

/// An XML document and the arena that owns its nodes and bytes.
///
/// `nodes[i]` is the node whose `NodeId.index` is `i`; the byte store holds
/// every name and value the nodes span. Keeping byte *offsets* rather than
/// pointers means a growing `Vec` never invalidates a node.
pub struct Document {
    pub(crate) nodes: Vec<Node>,
    pub(crate) bytes: Vec<u8>,
    /// The reserved `xml:` / `xmlns:` URIs, stored once so bindings can name
    /// them by span like any other URI.
    pub(crate) xml_ns: Span,
    pub(crate) xmlns_ns: Span,
    /// This document's unique stamp, copied into every node's `generation` so a
    /// `NodeId` from another document is rejected by `try_node`.
    pub(crate) stamp: u32,
    /// Running total counted against `max_bytes` (nodes + bytes).
    pub arena_bytes: usize,
    pub max_bytes: usize,
    pub max_nodes: usize,
    pub oom: i32,
    pub root: Option<NodeId>,
    pub doc_node: NodeId,
    pub doctype: Option<NodeId>,
    /// Rust-owned cache; mutation drops it before changing links.
    pub(crate) name_index: Option<Box<crate::xml::index::NameIndex>>,
    pub has_encoding_decl: i32,
}

/// Historical name for [`Document`]; the Ruby glue and XPath backend refer to
/// the document type by this.
pub type Doc = Document;

impl Document {
    pub(crate) fn blank() -> Self {
        Document {
            nodes: Vec::new(),
            bytes: Vec::new(),
            xml_ns: Span::EMPTY,
            xmlns_ns: Span::EMPTY,
            stamp: 0,
            arena_bytes: 0,
            max_bytes: MAX_BYTES,
            max_nodes: MAX_NODES,
            oom: 0,
            root: None,
            doc_node: NodeId::INVALID,
            doctype: None,
            name_index: None,
            has_encoding_decl: 0,
        }
    }
}

/// The C `""` sentinel: a valid, non-NULL, NUL-terminated empty string that a
/// zero-length slice may point at (never read past, never freed). Retained for
/// the handful of FFI out-parameters that still hand a `*const c_char` back.
pub static EMPTY: [u8; 1] = [0];

#[inline]
pub fn empty() -> *const c_char {
    EMPTY.as_ptr() as *const c_char
}

/// View a C (ptr,len) pair as a byte slice. NULL or len 0 is the empty slice,
/// so a "" / NULL field never gets dereferenced.
#[inline]
pub unsafe fn bytes<'a>(p: *const c_char, len: u32) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p as *const u8, len as usize)
    }
}
