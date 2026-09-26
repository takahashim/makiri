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
/// variant: success is `Ok`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParseError {
    Syntax,
    Limit,
    Oom,
    Internal,
    /// Well-formed, but it uses a DTD construct Makiri does not apply (an
    /// attribute default, a non-CDATA attribute type, a parameter entity, a
    /// reference to a declared entity) - refused rather than ignored.
    Unsupported,
}

/// Why a bounded allocation was refused: a document's own budget (`max_bytes` /
/// `max_nodes`), a stack's cap (`MAX_NS`), or the machine's memory.
///
/// Both the arena and the namespace-scope stack answer one of these two and
/// nothing else, so each caller's conversion - [`ParseError`] for the parser,
/// [`MutError`] for a mutation - is exhaustive rather than a guess.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BudgetError {
    Limit,
    Oom,
}

impl From<BudgetError> for ParseError {
    #[inline]
    fn from(e: BudgetError) -> Self {
        match e {
            BudgetError::Limit => ParseError::Limit,
            BudgetError::Oom => ParseError::Oom,
        }
    }
}

impl From<BudgetError> for MutError {
    #[inline]
    fn from(e: BudgetError) -> Self {
        match e {
            BudgetError::Limit => MutError::Limit,
            BudgetError::Oom => MutError::Oom,
        }
    }
}

/* ---- node kinds ---- */

/// An arena node's kind. The discriminants are the WHATWG DOM numbers - the
/// same value Ruby's `#node_type` returns and the same set
/// [`crate::node_type::NodeType`] names - so the conversion at the engine
/// boundary is the identity on the number. Entity / entity-reference /
/// notation (5, 6, 12) have no Makiri node and are not representable.
///
/// Named `ArenaKind`, not `NodeType`: the crate-wide
/// [`NodeType`](crate::node_type::NodeType) is what every other layer reads,
/// and the two used to share a name while spelling the variants differently.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u32)]
pub enum ArenaKind {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CDataSection = 4,
    Pi = 7,
    Comment = 8,
    Document = 9,
    DocumentType = 10,
    DocumentFragment = 11,
}

impl From<ArenaKind> for crate::node_type::NodeType {
    /// The crate-wide [`NodeType`](crate::node_type::NodeType) of an arena
    /// node - the one the XPath engine and the Ruby class table read. The two
    /// enums share the DOM discriminants, so this is the identity on the
    /// number; XML simply has no entity, entity-reference or notation node to
    /// map.
    #[inline]
    fn from(t: ArenaKind) -> Self {
        use crate::node_type::NodeType as N;
        match t {
            ArenaKind::Element => N::Element,
            ArenaKind::Attribute => N::Attribute,
            ArenaKind::Text => N::Text,
            ArenaKind::CDataSection => N::CDataSection,
            ArenaKind::Pi => N::Pi,
            ArenaKind::Comment => N::Comment,
            ArenaKind::Document => N::Document,
            ArenaKind::DocumentType => N::DocumentType,
            ArenaKind::DocumentFragment => N::DocumentFragment,
        }
    }
}

/// An ELEMENT's state bits. A set of named bits rather than a bare integer, so
/// a site says which state it tests, sets or clears instead of spelling the
/// mask. An ATTRIBUTE's namespace state is a separate [`AttrNs`], not a bit
/// here: its three values are exclusive.
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

/// An ATTRIBUTE's namespace state - one of three, never a combination:
///
/// - [`AttrNs::Derived`]: the namespace comes from the prefix, resolved against
///   the in-scope declarations when the attribute's element is.
/// - [`AttrNs::Pending`]: the prefix was unbound when the attribute was named -
///   on a detached element, where that defers rather than fails. Its namespace
///   reads empty only because nothing has decided it, which is not the same as
///   "no namespace": the insertion that connects its element resolves it (and
///   is refused if the prefix is still unbound), even under an element whose
///   own namespace was decided long before. Without this state the two were
///   one, and a removed-then-edited element came back with `ns1:a` bound to "".
/// - [`AttrNs::Explicit`]: the namespace was GIVEN (`set_attribute_ns`) rather
///   than derived from its prefix. Resolution leaves it alone: re-deriving it
///   when a detached element was inserted put `set_attribute_ns("urn:a", "x")`
///   in no namespace (an unprefixed name resolves to none), and a `q:x` into
///   whatever `q` meant at the insertion point. Naming the attribute again by
///   its qualified name alone returns it to `Derived`.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum AttrNs {
    #[default]
    Derived,
    Pending,
    Explicit,
}

/* ---- mutation status ---- */

/// Why a tree mutation failed - the `Err` of every mutator's `Result`. Each
/// variant names a rule the mutation broke, which the glue maps to a Ruby
/// exception. There is deliberately no success variant: success is `Ok`.
/// Kept distinct from [`ParseError`] because the failure domains differ (a mutation
/// never fails with [`ParseError::Syntax`], a parse never with
/// [`MutError::Cycle`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MutError {
    Oom,
    BadName,
    BadChars,
    UnboundNs,
    Type,
    Cycle,
    Hierarchy,
    BadNsDecl(crate::xml::qname::NsDeclError),
    /// A null / stale document handle reached a mutator; its own variant, so
    /// it cannot be mistaken for [`MutError::UnboundNs`].
    Internal,
    /// The document's OWN budget refused the allocation - `max_bytes` or
    /// `max_nodes` - which is not the machine running out of memory.
    ///
    /// It exists because without it every arena failure collapsed into
    /// [`MutError::Oom`] at ~30 call sites, so filling a document's byte
    /// budget told the caller "out of memory mutating XML" on a machine with
    /// gigabytes free. The parse path always kept them apart
    /// ([`ParseError::Limit`] -> `Makiri::XML::LimitExceeded`); mutation now does
    /// too. `From<BudgetError>` is the one conversion from an arena refusal.
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
/// an `&[NodeId]` is layout-identical to the engine's node-set buffer, and
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
    /// The absent handle. Its token word is 0, which is the engine's "no
    /// node" ([`crate::token::Token`]'s null slot), so index 0 is reserved and
    /// never a real node.
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
    pub type_: ArenaKind,
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
    pub attr_ns: AttrNs,
}

/* A node is the arena's unit of cost (`NODE_COST`), so its size is pinned:
 * `Option<Link>` must stay 4 bytes. */
const _: () = assert!(core::mem::size_of::<Node>() == 80);

impl Node {
    pub(crate) fn zeroed(type_: ArenaKind) -> Self {
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
            attr_ns: AttrNs::Derived,
        }
    }
}

/// The per-document allocation limit. `None` is the default budget,
/// [`MAX_BYTES`].
pub struct ParseLimits {
    pub max_bytes: Option<usize>,
}

impl ParseLimits {
    /// The effective byte budget: `max_bytes`, or [`MAX_BYTES`]. The ONE place
    /// the default is resolved, so the parse, the fragment parse and the
    /// decode that runs before either cannot disagree.
    pub fn budget(&self) -> usize {
        self.max_bytes.unwrap_or(MAX_BYTES)
    }
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
    pub(super) arena_bytes: usize,
    pub(super) max_bytes: usize,
    pub(super) max_nodes: usize,
    pub(super) root: Option<NodeId>,
    pub(super) doc_node: NodeId,
    pub(super) doctype: Option<NodeId>,
    /// Rust-owned cache; mutation drops it before changing links.
    pub(crate) name_index: core::cell::OnceCell<Box<crate::xml::index::NameIndex>>,
    pub(super) has_encoding_decl: bool,
}
