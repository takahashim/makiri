//! The node-access contract the engine is written against.
//!
//! In C this is a pair of headers of `MKR_NODE_*` macros
//! (mkr_xpath_node_access_{html,xml}.h) plus a prelude that `#include`s the
//! engine bodies once per representation. That is monomorphization by
//! preprocessor: it costs nothing at runtime (a runtime kind-branch measured
//! ~+150%), but nothing checks that the two bindings agree on what an operation
//! means, or that the body only uses operations both provide.
//!
//! A trait is the same monomorphization with those two things checked. The
//! engine is generic over `Dom`, so each backend is compiled into its own copy
//! with the calls inlined - and a backend that forgets an operation, or gives it
//! the wrong type, does not build.

#![forbid(unsafe_code)]

use super::abi::*;
use crate::token::Token;

/* ---- node types (shared numeric encoding) ----
 *
 * The whole monomorphization rests on the two representations agreeing on the
 * node-type encoding, so a node's `type` integer means the same thing whichever
 * backend walks it. The values below are Lexbor's LXB_DOM_NODE_TYPE_*, and
 * `dom_html` asserts that equality at compile time. */
pub const NTYPE_ELEMENT: u32 = 1;
pub const NTYPE_ATTRIBUTE: u32 = 2;
pub const NTYPE_TEXT: u32 = 3;
pub const NTYPE_CDATA_SECTION: u32 = 4;
pub const NTYPE_ENTITY_REFERENCE: u32 = 5;
pub const NTYPE_ENTITY: u32 = 6;
pub const NTYPE_PI: u32 = 7;
pub const NTYPE_COMMENT: u32 = 8;
pub const NTYPE_DOCUMENT: u32 = 9;
pub const NTYPE_DOCUMENT_TYPE: u32 = 10;
pub const NTYPE_NOTATION: u32 = 12;

/// A document the evaluator reads, borrowed for `'d`.
///
/// `Self` is the borrow itself - a Lexbor document handle for HTML, an
/// `&xml::Document` for XML - so every node it hands out, and every name, value
/// or namespace slice, lives no longer than the document is lent. A link that
/// is not there (no parent, no next sibling, no attribute) is `None`, never a
/// null handle.
///
/// The evaluator holds the document for one evaluate, and it does not change in
/// that time: without a handler no Ruby runs, and with one the document refuses
/// every mutation until the evaluate returns (`glue::doc::DocumentEvaluation`).
///
/// The token boundary is safe on both sides: [`token`](Self::token) erases a
/// node this backend just lent, and [`resolve_token`](Self::resolve_token) reads
/// one back. A token is only ever made by this method or by the Ruby bridge
/// (which has checked the node's document), so reading one back is sound.
pub trait Dom<'d>: Copy {
    /// Selects the host-policy branches the C spells `#ifdef MKR_HOST_XML`:
    /// `id()` is the empty node-set in XML (an ID is DTD-declared, and DTDs are
    /// rejected at parse), `lang()` reads xml:lang rather than HTML's `lang`,
    /// and the CSS-lowered of-type hooks exist only for XML.
    const IS_XML: bool;

    type Node: Copy + Eq + 'd;

    /// An attribute node, as its own type: holding one is the proof it is an
    /// attribute, so the attribute readers take it without checking again.
    type Attr: Copy;

    /// The erased token a node-set stores for `n`.
    fn token(n: Self::Node) -> Token;

    /// The node a non-null token names.
    ///
    /// A token comes from [`token`](Self::token) or from the Ruby bridge, which
    /// checks the node's document first, so it always names a node of this
    /// document.
    fn resolve_token(self, t: Token) -> Self::Node;

    /// The document node itself, for a walk rooted at the whole tree.
    fn document_node(self) -> Self::Node;

    /// Refresh whatever the backend reads once per walk, before it starts.
    ///
    /// The HTML backend builds (or, after a mutation since the last evaluate,
    /// rebuilds) its element/attribute index here - building it also backfills
    /// each attribute's parent, which the parent and ancestor axes read. XML has
    /// no such state. `false` when it cannot be built (out of memory), and the
    /// evaluate fails closed.
    fn prepare(&self) -> bool {
        true
    }

    fn node_type(self, n: Self::Node) -> u32;

    /* navigation */
    fn first_child(self, n: Self::Node) -> Option<Self::Node>;
    fn last_child(self, n: Self::Node) -> Option<Self::Node>;
    fn next(self, n: Self::Node) -> Option<Self::Node>;
    fn prev(self, n: Self::Node) -> Option<Self::Node>;
    fn parent(self, n: Self::Node) -> Option<Self::Node>;

    /* attributes (the C contract's MKR_ELEM_FIRST_ATTR / MKR_ATTR_NEXT). Only an
     * element has any, so `first_attr` is also the element test; the rest take
     * an attribute handle and do not test again. */
    fn first_attr(self, el: Self::Node) -> Option<Self::Attr>;
    fn attr_next(self, a: Self::Attr) -> Option<Self::Attr>;
    /// The node an attribute handle is, for walking, pushing or comparing.
    fn attr_node(a: Self::Attr) -> Self::Node;
    /// `n` as an attribute, or None when it is some other kind of node.
    fn as_attr(self, n: Self::Node) -> Option<Self::Attr>;
    fn attr_value(self, a: Self::Attr) -> &'d [u8];
    /// Attribute value by raw qualified name, or None.
    fn get_attribute(self, el: Self::Node, name: &[u8]) -> Option<&'d [u8]>;

    /* names, borrowed from the document; empty for a node of the wrong kind */
    fn local_name(self, n: Self::Node) -> &'d [u8];
    fn attr_local_name(self, a: Self::Attr) -> &'d [u8];
    fn qualified_name(self, n: Self::Node) -> &'d [u8];
    fn attr_qualified_name(self, a: Self::Attr) -> &'d [u8];
    fn pi_name(self, n: Self::Node) -> &'d [u8];

    /// The node's namespace URI, empty if it has none.
    fn ns_uri(self, n: Self::Node) -> &'d [u8];

    /// True when a strict unprefixed element name test must NOT match this
    /// node: XML calls any namespace foreign, HTML admits its own and none.
    fn is_foreign_ns(self, n: Self::Node) -> bool;

    /// Whether the node is in a namespace at all - `MKR_NODE_NS_ID(n) != 0`.
    /// Separate from `ns_uri` because HTML answers it without the document.
    fn has_ns(self, n: Self::Node) -> bool;

    /// Append the node's own text - the bytes it contributes to a string-value -
    /// to `buf`; the error is the buffer's (its cap, or OOM).
    ///
    /// It appends rather than returning a slice because only one backend can
    /// lend those bytes. The XML node owns its value, but Lexbor builds a node's
    /// text content on demand and hands back an allocation the caller must free,
    /// so a borrowed return has nowhere to free it. Owning the append is the one
    /// shape both can satisfy - and it is what the C contract says
    /// (`MKR_NODE_APPEND_OWN_TEXT`, which is a statement, not an expression, for
    /// exactly this reason).
    fn append_own_text(self, n: Self::Node, buf: &mut Buf) -> Result<(), BufError>;

    /// The document-level element index's answer for a document-rooted,
    /// predicate-free descendant name test, or None when it cannot serve one.
    ///
    /// Both hosts keep such an index, but they key it differently: XML by
    /// (local name, namespace URI), which is exactly the test, and HTML by
    /// Lexbor tag id, which is only an approximation - hence `recheck`.
    fn name_bucket(
        self,
        local: &[u8],
        ns_uri: Option<&[u8]>,
        lax: bool,
    ) -> Option<Bucket<'d, Self::Node>>;
}

/// What `Dom::name_bucket` found: the elements, in document order, and whether
/// each still has to be re-checked against the name test before it counts.
pub struct Bucket<'a, N> {
    pub nodes: &'a [N],
    pub recheck: bool,
}
