//! The node-access contract the engine is written against.
//!
//! The C engine this replaced bound its two representations with per-host
//! macro headers, `#include`-ing the engine bodies once per representation.
//! That is monomorphization by preprocessor: it costs nothing at runtime (a runtime kind-branch measured
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

/// A node's type, as the engine reads it: the crate's one DOM node-type enum,
/// shared with the HTML adapter and the XML model.
pub use crate::node_type::NodeType;

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
/// every mutation until the evaluate returns (`bridge::wrapper::DocumentEvaluation`).
///
/// The token boundary is safe on both sides: [`token`](Self::token) erases a
/// node this backend just lent, and [`resolve_token`](Self::resolve_token) reads
/// one back. A token is only ever made by this method or by the Ruby bridge
/// (which has checked the node's document), so reading one back is sound.
///
/// # Host policy
///
/// Where XPath over HTML and XPath over XML answer differently, the difference
/// is one of the items under "host policy" below, each named for the question
/// the engine asks. The engine never asks WHICH host it is walking: a policy
/// that is not an item here is not a policy, and a new host states each one in
/// its `impl`.
pub trait Dom<'d>: Copy {
    type Node: Copy + Eq + 'd;

    /// An attribute node, as its own type: holding one is the proof it is an
    /// attribute, so the attribute readers take it without checking again.
    type Attr: Copy;

    /// The erased token a node-set stores for `n`.
    fn token(n: Self::Node) -> Token;

    /// The node a token names.
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
    /// rebuilds) its element index here. XML has no such state. `Err` when it
    /// cannot be built (out of memory), and the evaluate fails closed.
    fn prepare(&self) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn node_type(self, n: Self::Node) -> NodeType;

    /* navigation */
    fn first_child(self, n: Self::Node) -> Option<Self::Node>;
    fn last_child(self, n: Self::Node) -> Option<Self::Node>;
    fn next(self, n: Self::Node) -> Option<Self::Node>;
    fn prev(self, n: Self::Node) -> Option<Self::Node>;
    fn parent(self, n: Self::Node) -> Option<Self::Node>;

    /* attributes. Only an element has any, so `first_attr` is also the element test; the rest take
     * an attribute handle and do not test again. */
    fn first_attr(self, el: Self::Node) -> Option<Self::Attr>;
    fn attr_next(self, a: Self::Attr) -> Option<Self::Attr>;

    /// `el`'s attributes, in the host's order: [`first_attr`](Dom::first_attr)
    /// then [`attr_next`](Dom::attr_next). Empty for anything but an element,
    /// since `first_attr` is the element test too - the way to walk them.
    #[inline]
    fn attributes(self, el: Self::Node) -> impl Iterator<Item = Self::Attr> {
        core::iter::successors(self.first_attr(el), move |&a| self.attr_next(a))
    }
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

    /// The node's namespace URI, empty if it has none. For an attribute, ask
    /// [`attr_ns_uri`](Self::attr_ns_uri) - see there.
    fn ns_uri(self, n: Self::Node) -> &'d [u8];

    /// Whether name tests compare this element's names ASCII case-insensitively:
    /// an HTML element in an HTML document, as browsers do (WPT domxpath
    /// text-html-*). Its own attributes follow it; an SVG / MathML element, and
    /// every XML node, compares exactly.
    fn folds_name_case(self, el: Self::Node) -> bool;

    /// Whether the element is in a namespace at all. Separate from `ns_uri`
    /// because HTML answers it without the document.
    fn has_ns(self, n: Self::Node) -> bool;

    /* ---- host policy ---- */

    /// The name a name test compares for element `n`. A prefixed test compares
    /// the local name (the namespace is compared on its own). An unprefixed
    /// one compares the local name in XML, where the namespace rule decides the
    /// rest, and the QUALIFIED name in HTML, as browsers do.
    fn test_name(self, n: Self::Node, prefixed: bool) -> &'d [u8];

    /// [`test_name`](Self::test_name) for an attribute.
    fn attr_test_name(self, a: Self::Attr, prefixed: bool) -> &'d [u8];

    /// Whether an unprefixed name test that matched `n`'s name matches `n`:
    /// an element, or an attribute node when `is_attr`.
    ///
    /// Strict (`lax` false) is the specification: XML admits no namespace only;
    /// HTML admits an element in the HTML namespace or none - a foreign (SVG /
    /// MathML) element needs a prefix, as in browsers - and every attribute,
    /// whose qualified-name compare already set the prefixed ones apart.
    ///
    /// Lax (`namespace_matching: :lax`) is Nokogiri's behaviour, whatever that
    /// is for the host: `Nokogiri::HTML` has no namespaces, so HTML admits any
    /// element; `Nokogiri::XML` (libxml2) is namespace-strict, so XML ignores
    /// the flag.
    fn unprefixed_matches(self, n: Self::Node, is_attr: bool, lax: bool) -> bool;

    /// An attribute's OWN namespace URI, empty when it has none - never its
    /// element's. The XPath data model and the DOM agree: `id` on an HTML
    /// `<div>` is in no namespace, `xlink:href` on an SVG element is in XLink's.
    fn attr_ns_uri(self, a: Self::Attr) -> &'d [u8];

    /// The attribute `id()` looks an ID up in, or None when the host has no ID
    /// attributes: in XML an ID is an attribute a DTD declares ID-typed, and
    /// DTDs are refused at parse, so `id()` is the empty node-set there.
    const ID_ATTRIBUTE: Option<&'static [u8]>;

    /// The attributes `lang()` reads on each ancestor, in order: XPath 1.0's
    /// `xml:lang`, and for HTML its own `lang` first.
    const LANG_ATTRIBUTES: &'static [&'static [u8]];

    /// Append the node's own text - the bytes it contributes to a string-value -
    /// to `buf`; the error is the buffer's (its cap, or OOM).
    ///
    /// It appends rather than returning a slice because only one backend can
    /// lend those bytes. The XML node owns its value, but Lexbor builds a node's
    /// text content on demand and hands back an allocation the caller must free,
    /// so a borrowed return has nowhere to free it. Owning the append is the one
    /// shape both can satisfy.
    fn append_own_text(self, n: Self::Node, buf: &mut Buf) -> Result<(), BufError>;

    /// The document-level element index's answer for a document-rooted,
    /// predicate-free descendant name test, or None when it cannot serve one.
    ///
    /// Both hosts keep such an index, but they key it differently: XML by
    /// (local name, namespace URI), which is exactly the test, and HTML by
    /// Lexbor tag id, which is only an approximation - hence `recheck`.
    ///
    /// An unprefixed test is the strict one; where lax admits more (HTML's
    /// foreign elements), the host must not answer a bucket that leaves them
    /// out.
    fn name_bucket(self, local: &[u8], ns_uri: Option<&[u8]>) -> Option<Bucket<'d, Self::Node>>;
}

/// What `Dom::name_bucket` found: the elements, in document order, and whether
/// each still has to be re-checked against the name test before it counts.
pub struct Bucket<'a, N> {
    pub nodes: &'a [N],
    pub recheck: bool,
}
