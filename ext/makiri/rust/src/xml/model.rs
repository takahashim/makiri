//! The XML node model: the status and node-type enums, the index-based node and
//! document layouts, and the handle types shared with the XPath backend and the
//! Ruby glue.
//!
//! The tree is an **index arena**: a [`Document`] owns a `Vec<Node>` and a byte
//! store, a node *handle* is a [`NodeId`] (slot index plus the owning
//! document's stamp), structural links are compact [`Link`]s (a slot index
//! only), and every name/value is an `(offset, len)` into the document's byte
//! store. No raw pointer is part of the model, so the parser, mutators, index
//! and XPath XML backend can all be ordinary safe Rust.

/* Boundary readers state their precondition once, on `bytes`. */
/* ---- status codes ---- */

/// The outcome of an XML operation that reports failure through a status rather
/// than a typed error: parsing, tree building and arena allocation all
/// accumulate one. [`Status::Ok`] is success; the rest name the failure so the
/// Ruby glue can pick an exception class. The `#[repr(i32)]` values are the
/// numbers the C entry points published.
///
/// `Ok` is a member (the parser and the arena keep a sticky status that starts
/// there) but a `Result<T, Status>` never carries it in the `Err` position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum Status {
    Ok = 0,
    Syntax = 1,
    Limit = 2,
    Oom = 3,
    Internal = 4,
    Version = 5,
}

impl Status {
    /// Whether this is the success status (the common `status != Ok` test).
    #[inline]
    pub fn is_ok(self) -> bool {
        matches!(self, Status::Ok)
    }
}

/* ---- node types ---- */

/// A DOM node type (`Node.nodeType`). The discriminants are the WHATWG DOM
/// numbers - the same value Ruby's `#node_type` returns and the same set the
/// XPath engine's `NTYPE_*` constants name - so converting to `u32` at those
/// two boundaries is a plain cast. Entity / entity-reference / notation (5, 6,
/// 12) have no Makiri node and are not representable.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u32)]
pub enum NodeType {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CData = 4,
    Pi = 7,
    Comment = 8,
    Document = 9,
    Doctype = 10,
    Fragment = 11,
}

impl NodeType {
    #[inline]
    pub fn as_u32(self) -> u32 {
        self as u32
    }
}

impl TryFrom<u32> for NodeType {
    type Error = ();
    /// The inverse of [`NodeType::as_u32`], failing on anything the DOM does not
    /// define (including the unused 5/6/12) so a bad value can never be stored.
    #[inline]
    fn try_from(v: u32) -> Result<Self, ()> {
        Ok(match v {
            1 => NodeType::Element,
            2 => NodeType::Attribute,
            3 => NodeType::Text,
            4 => NodeType::CData,
            7 => NodeType::Pi,
            8 => NodeType::Comment,
            9 => NodeType::Document,
            10 => NodeType::Doctype,
            11 => NodeType::Fragment,
            _ => return Err(()),
        })
    }
}

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

/* ---- mutation status ---- */

/// The outcome of a tree mutation. [`MutStatus::Ok`] is success; each failure
/// names a rule the mutation broke, which the glue maps to a Ruby exception.
/// Kept distinct from [`Status`] because the failure domains differ (a mutation
/// never fails with [`Status::Syntax`], a parse never with
/// [`MutStatus::Cycle`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum MutStatus {
    Ok = 0,
    Oom = 1,
    BadName = 2,
    BadChars = 3,
    UnboundNs = 4,
    Type = 5,
    Cycle = 6,
    Hierarchy = 7,
    BadNsDecl = 8,
    /// A null / stale document handle reached a mutator. The C entry points
    /// overloaded the parse code `4` here; as its own variant it can no longer
    /// be mistaken for [`MutStatus::UnboundNs`].
    Internal = 9,
}

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
/// 32 bits are the slot index, the high 32 the document [`Document::stamp`].
///
/// The one-word form is deliberate. The engine carries nodes through node-sets
/// as opaque tokens it only compares and hashes, so a node id *is* that token:
/// an `&[NodeId]` is layout-identical to the engine's `*mut c_void` buffer, and
/// `to_token`/`from_token` cost nothing (`#[repr(transparent)]` makes that
/// layout a guarantee, not an observation). The packing needs a 64-bit word,
/// which the `compile_error!` below enforces.
///
/// The high half is the owning document's stamp, NOT a slot-reuse generation:
/// slots are never recycled (detach never destroys, so a removed node stays
/// addressable for live Ruby wrappers), so there is no reuse tag to carry.
/// Using it as a document stamp instead lets [`Document::try_node`] reject a
/// handle built for another document. If slots are ever recycled this becomes
/// index + document/reuse stamp, and [`Document::try_node`] already checks it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
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
    pub(crate) fn new(index: u32, stamp: u32) -> Self {
        NodeId(((stamp as usize) << 32) | index as usize)
    }
    #[inline]
    pub fn index(self) -> u32 {
        self.0 as u32
    }
    /// The owning document's stamp (see [`Document::stamp`]).
    #[inline]
    pub fn stamp(self) -> u32 {
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

/// A structural link: the slot index of a parent / child / sibling / first
/// attribute, or [`Link::NONE`] (slot 0, the reserved null slot) when absent.
///
/// Links are 4 bytes and travel inside exactly one [`Document`], so they carry
/// no stamp; the owning document re-attaches its [`Document::stamp`] when a link
/// is handed back out as a [`NodeId`]. A link to the invalid handle
/// ([`NodeId::INVALID`], index 0) is [`Link::NONE`], which is the correct
/// reading: an invalid node has no link.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Link(u32);

impl Link {
    /// No link (also the encoding of a link to [`NodeId::INVALID`]).
    pub(crate) const NONE: Link = Link(0);

    /// The link naming `id`; [`NodeId::INVALID`] (index 0) becomes
    /// [`Link::NONE`].
    #[inline]
    pub(crate) fn of(id: NodeId) -> Self {
        Link(id.index())
    }
    /// As [`Link::of`], for an optional handle.
    #[inline]
    pub(crate) fn from_option(id: Option<NodeId>) -> Self {
        Link(id.map_or(0, |id| id.index()))
    }
    /// This link as an `Option`, `None` for [`Link::NONE`].
    #[inline]
    pub(crate) fn optional(self) -> Option<Link> {
        if self.is_none() {
            None
        } else {
            Some(self)
        }
    }
    /// The slot index this link names (0 = none).
    #[inline]
    pub(crate) fn index(self) -> u32 {
        self.0
    }
    #[inline]
    pub(crate) fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// One node in the document's slot array.
///
/// Links are compact [`Link`]s and byte fields are spans, so a `Node` is plain
/// data with no pointer to chase and no per-node document stamp (the stamp lives
/// once on the [`Document`]).
pub struct Node {
    pub type_: NodeType,
    pub parent: Link,
    pub first_child: Link,
    pub last_child: Link,
    pub prev: Link,
    pub next: Link,
    pub attrs: Link,
    pub qname: Span,
    pub local: Span,
    pub prefix: Span,
    pub ns_uri: Span,
    pub value: Span,
    pub line: u32,
    pub col: u32,
    pub flags: u32,
}

impl Node {
    pub(crate) fn zeroed(type_: NodeType) -> Self {
        Node {
            type_,
            parent: Link::NONE,
            first_child: Link::NONE,
            last_child: Link::NONE,
            prev: Link::NONE,
            next: Link::NONE,
            attrs: Link::NONE,
            qname: Span::ABSENT,
            local: Span::ABSENT,
            prefix: Span::ABSENT,
            ns_uri: Span::ABSENT,
            value: Span::ABSENT,
            line: 0,
            col: 0,
            flags: 0,
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
    /// This document's unique stamp, carried in every `NodeId` the document
    /// issues (the high half) so [`Document::try_node`] rejects a handle built
    /// for another document. Node links carry only the slot index.
    pub(crate) stamp: u32,
    /// Running total counted against `max_bytes` (nodes + bytes).
    pub arena_bytes: usize,
    pub max_bytes: usize,
    pub max_nodes: usize,
    /// The first failure the arena hit, sticky until the document is dropped.
    /// [`Status::Ok`] until something fails.
    pub status: Status,
    pub root: Option<NodeId>,
    pub doc_node: NodeId,
    pub doctype: Option<NodeId>,
    /// Rust-owned cache; mutation drops it before changing links.
    pub(crate) name_index: Option<Box<crate::xml::index::NameIndex>>,
    pub has_encoding_decl: bool,
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
            status: Status::Ok,
            root: None,
            doc_node: NodeId::INVALID,
            doctype: None,
            name_index: None,
            has_encoding_decl: false,
        }
    }
}
