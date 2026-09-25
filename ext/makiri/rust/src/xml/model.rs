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

#![forbid(unsafe_code)]

use core::num::NonZeroU32;

/* Boundary readers state their precondition once, on `bytes`. */
/* ---- status codes ---- */

/// Why an XML parse failed - the `Err` of every parse entry point, and of the
/// parser's internal steps, which carry it out with `?`. Each variant names a
/// failure so the Ruby glue can pick an exception class. There is no success
/// variant: success is `Ok`. The `#[repr(i32)]` values are the numbers the C
/// entry points published.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum Status {
    Syntax = 1,
    Limit = 2,
    Oom = 3,
    Internal = 4,
    /// Well-formed, but it uses a DTD construct Makiri does not apply (an
    /// attribute default, a non-CDATA attribute type, a parameter entity, a
    /// reference to a declared entity) - refused rather than ignored.
    Unsupported = 5,
}

/// Why the arena refused an allocation: the document's own budget
/// (`max_bytes` / `max_nodes`), or the machine's memory. Every arena
/// allocation answers one of these two and nothing else, so each caller's
/// conversion - [`Status`] for the parser, [`MutStatus`] for a mutation - is
/// exhaustive rather than a guess.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArenaError {
    Limit,
    Oom,
}

impl From<ArenaError> for Status {
    #[inline]
    fn from(e: ArenaError) -> Self {
        match e {
            ArenaError::Limit => Status::Limit,
            ArenaError::Oom => Status::Oom,
        }
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

/// A node's state bits: [`NodeFlags::DOM_LOOSE_NAME`] and the three namespace
/// states. A set of named bits rather than a bare integer, so a site says which
/// state it tests, sets or clears instead of spelling the mask.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct NodeFlags(u8);

impl NodeFlags {
    /// No state.
    pub const EMPTY: NodeFlags = NodeFlags(0);

    /// Set on an element built by `create_loose_dom_element`: its name is a
    /// WHATWG DOM name that need not be an XML QName, so the serializer refuses
    /// to write it (see [`crate::xml::dom_name`]).
    pub const DOM_LOOSE_NAME: NodeFlags = NodeFlags(0x01);

    /// Set on an ELEMENT once its namespace URI has been decided - by the parser,
    /// or by resolving it against the context it was first inserted into. From
    /// then on the URI is the node's IDENTITY, not a value derived from the
    /// declarations around it: moving the node does not change it, and the
    /// serializer emits whatever declarations the output needs to reproduce it
    /// (the WHATWG DOM model, matching what browsers do). An element still
    /// carrying no flag - freshly built by a factory - has no namespace yet and
    /// takes one from its insertion context, so building a subtree bottom-up and
    /// attaching it gives the same tree as building it top-down.
    pub const NS_RESOLVED: NodeFlags = NodeFlags(0x02);

    /// Set on an ATTRIBUTE whose prefix was unbound when it was named - on a
    /// detached element, where that defers rather than fails. Its namespace reads
    /// empty only because nothing has decided it, which is not the same as "no
    /// namespace": the insertion that connects its element resolves it (and is
    /// refused if the prefix is still unbound), even under an element whose own
    /// namespace was decided long before. Without the flag the two were one state,
    /// and a removed-then-edited element came back with `ns1:a` bound to "".
    pub const NS_PENDING: NodeFlags = NodeFlags(0x04);

    /// Set on an ATTRIBUTE whose namespace was GIVEN (`set_attribute_ns`) rather
    /// than derived from its prefix. Resolution leaves it alone: re-deriving it
    /// when a detached element was inserted put `set_attribute_ns("urn:a", "x")`
    /// in no namespace (an unprefixed name resolves to none), and a `q:x` into
    /// whatever `q` meant at the insertion point. Naming the attribute again by
    /// its qualified name alone clears it.
    pub const NS_EXPLICIT: NodeFlags = NodeFlags(0x08);

    /// Whether every bit of `f` is set.
    #[inline]
    pub fn contains(self, f: NodeFlags) -> bool {
        self.0 & f.0 == f.0
    }
    #[inline]
    pub fn insert(&mut self, f: NodeFlags) {
        self.0 |= f.0;
    }
    #[inline]
    pub fn remove(&mut self, f: NodeFlags) {
        self.0 &= !f.0;
    }
    /// [`NodeFlags::insert`] when `on`, else [`NodeFlags::remove`].
    #[inline]
    pub fn set(&mut self, f: NodeFlags, on: bool) {
        if on {
            self.insert(f);
        } else {
            self.remove(f);
        }
    }
}

/* ---- mutation status ---- */

/// Why a tree mutation failed - the `Err` of every mutator's `Result`. Each
/// variant names a rule the mutation broke, which the glue maps to a Ruby
/// exception. There is deliberately no success variant: success is `Ok`.
/// Kept distinct from [`Status`] because the failure domains differ (a mutation
/// never fails with [`Status::Syntax`], a parse never with
/// [`MutStatus::Cycle`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MutStatus {
    Oom,
    BadName,
    BadChars,
    UnboundNs,
    Type,
    Cycle,
    Hierarchy,
    BadNsDecl(crate::xml::qname::NsDeclError),
    /// A null / stale document handle reached a mutator; its own variant, so
    /// it cannot be mistaken for [`MutStatus::UnboundNs`].
    Internal,
    /// The document's OWN budget refused the allocation - `max_bytes` or
    /// `max_nodes` - which is not the machine running out of memory.
    ///
    /// It exists because without it every arena failure collapsed into
    /// [`MutStatus::Oom`] at ~30 call sites, so filling a document's byte
    /// budget told the caller "out of memory mutating XML" on a machine with
    /// gigabytes free. The parse path always kept them apart
    /// ([`Status::Limit`] -> `Makiri::XML::LimitExceeded`); mutation now does
    /// too. `mutate::arena` is the one conversion from [`ArenaError`].
    Limit,
    /// Another attribute of the element already has this (namespace URI, local
    /// name) - Namespaces in XML 1.0 §3's "attributes are unique", which the
    /// parser enforces. `[]=` by a second prefix for the same URI, or a rename
    /// onto another attribute's name, wrote two, and the output did not parse.
    DuplicateAttr,
    /// A namespace that does not fit the qualified name it was given with
    /// (the DOM's "validate and extract"): a prefix without a namespace, `xml`
    /// or `xmlns` with another one, or the XMLNS namespace on another name.
    BadNsName,
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
    pub fn to_token(self) -> usize {
        self.0
    }
    #[inline]
    pub(crate) fn from_token(token: usize) -> Self {
        NodeId(token)
    }
}

/// A structural link: the slot index of a parent / child / sibling / first
/// attribute. A node's link fields are `Option<Link>`, None when absent.
///
/// Links are 4 bytes and travel inside exactly one [`Document`], so they carry
/// no stamp; the owning document re-attaches its [`Document::stamp`] when a link
/// is handed back out as a [`NodeId`]. The index is never 0 - slot 0 is the
/// reserved null slot - which is what lets `Option<Link>` stay 4 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Link(NonZeroU32);

impl Link {
    /// The link naming `id`; [`NodeId::INVALID`] (index 0) names no node, so
    /// it has none.
    #[inline]
    pub(crate) fn of(id: NodeId) -> Option<Link> {
        NonZeroU32::new(id.index()).map(Link)
    }
    /// As [`Link::of`], for an optional handle.
    #[inline]
    pub(crate) fn from_option(id: Option<NodeId>) -> Option<Link> {
        id.and_then(Link::of)
    }
    /// The slot index this link names (never 0).
    #[inline]
    pub(crate) fn index(self) -> u32 {
        self.0.get()
    }
}

/// One node in the document's slot array.
///
/// Links are compact [`Link`]s and byte fields are spans, so a `Node` is plain
/// data with no pointer to chase and no per-node document stamp (the stamp lives
/// once on the [`Document`]).
pub struct Node {
    pub type_: NodeType,
    pub parent: Option<Link>,
    pub first_child: Option<Link>,
    pub last_child: Option<Link>,
    pub prev: Option<Link>,
    pub next: Option<Link>,
    pub attrs: Option<Link>,
    pub qname: Span,
    pub local: Span,
    pub prefix: Span,
    pub ns_uri: Span,
    pub value: Span,
    pub line: u32,
    pub col: u32,
    pub flags: NodeFlags,
}

/* A node is the arena's unit of cost (`NODE_COST`), so its size is pinned:
 * `Option<Link>` must stay 4 bytes. */
const _: () = assert!(core::mem::size_of::<Node>() == 80);

impl Node {
    pub(crate) fn zeroed(type_: NodeType) -> Self {
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
            flags: NodeFlags::EMPTY,
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
    pub root: Option<NodeId>,
    pub doc_node: NodeId,
    pub doctype: Option<NodeId>,
    /// Rust-owned cache; mutation drops it before changing links.
    pub(crate) name_index: core::cell::OnceCell<Box<crate::xml::index::NameIndex>>,
    pub has_encoding_decl: bool,
}

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
            root: None,
            doc_node: NodeId::INVALID,
            doctype: None,
            name_index: core::cell::OnceCell::new(),
            has_encoding_decl: false,
        }
    }
}
