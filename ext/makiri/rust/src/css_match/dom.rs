//! What the matcher reads of a DOM: the traits a host's node, element,
//! attribute and document handles implement, as XPath's `Dom` is what its
//! evaluator reads. The names are the HTML arena's handles' own
//! (`html::dom::handle`), whose impls only forward to them, so the matcher
//! reads as it did when it was written against those handles - and
//! monomorphizes to the same calls.
//!
//! What differs between hosts is a named item here, never a test of which
//! host the matcher walks: the namespace a node is in ([`CssNode::is_html`],
//! [`CssNode::same_namespace`]), its tag id ([`CssNode::tag_id`]), and the
//! resolved names and quirks mode of its document ([`CssDoc`]).

use crate::node_type::NodeType;

/// A DOM the matcher runs over: the types its [`Scratch`](super::Scratch)
/// keeps between queries, which name no document. `Node<'static>` is never
/// a live node - only the type a kept, emptied vector is held at between
/// queries (`scratch::recycle`).
pub trait Host: 'static + Sized {
    /// Whether names are resolved to ids once per walking query
    /// ([`CssDoc::tag_id`], [`CssDoc::resolve_attr_name`]); without, they
    /// are compared as bytes, as `matches?` compares them everywhere.
    const RESOLVES_NAMES: bool;
    type Node<'d>: CssNode<'d, Host = Self>;
    /// A type selector's name resolved in a document: equal ids are equal
    /// names with ASCII case folded.
    type TagId: Copy + PartialEq;
    /// An attribute selector's name resolved in a document, for
    /// [`CssElement::attr_by_resolved_name`].
    type AttrName: Copy;
}

/// A node, borrowed from its document for `'d`.
pub trait CssNode<'d>: Copy + PartialEq + 'd {
    type Host: Host;
    type Element: CssElement<'d, Node = Self, Host = Self::Host>;
    type Doc: CssDoc<'d, Host = Self::Host>;

    fn element(self) -> Option<Self::Element>;
    fn parent(self) -> Option<Self>;
    fn first_child(self) -> Option<Self>;
    fn next(self) -> Option<Self>;
    fn prev(self) -> Option<Self>;
    /// The next node after `self` in a preorder walk of `root`'s subtree.
    fn preorder_next(self, root: Self) -> Option<Self>;
    /// [`preorder_next`](Self::preorder_next), leaving `self`'s own
    /// subtree out.
    fn preorder_next_past(self, root: Self) -> Option<Self>;
    fn node_type(self) -> NodeType;
    /// A text, CDATA or comment node's data.
    fn char_data(self) -> Option<&'d [u8]>;
    /// Whether the node is in the HTML namespace: what the form-state
    /// pseudo-classes ask, in any document.
    fn is_html(self) -> bool;
    /// Whether the node is an HTML element in an HTML document, where a
    /// selector's names - and the HTML Standard's list of attribute values -
    /// compare ASCII case-insensitively ("case-sensitivity of selectors"). An
    /// XHTML element in an XML document is not.
    fn folds_case(self) -> bool;
    /// The node's namespace URI; `None` for no namespace.
    fn ns_uri(self) -> Option<&'d [u8]>;
    /// Whether the two nodes are in the same namespace (none counting as
    /// one).
    fn same_namespace(self, other: Self) -> bool;
    /// An element's tag id in its document, comparable with
    /// [`CssDoc::tag_id`]'s.
    fn tag_id(self) -> Option<<Self::Host as Host>::TagId>;
    fn owner_document(self) -> Self::Doc;
    /// Whether the node is its document's root element (`:root`).
    fn is_root(self) -> bool;
    /// The node as a key of a `ptr_table` map: equal exactly when the nodes
    /// are, for as long as the query runs.
    fn table_key(self) -> *const ();
}

/// An element.
pub trait CssElement<'d>: Copy {
    type Node: CssNode<'d>;
    type Host: Host;
    type Attr: CssAttr<'d>;
    type Attrs: Iterator<Item = Self::Attr>;

    fn node(self) -> Self::Node;
    /// The local name as the host stores it - ASCII lower-cased in HTML, as
    /// Lexbor keys names.
    fn local_name(self) -> &'d [u8];
    /// The DOM's `localName`.
    fn dom_local_name(self) -> &'d [u8];
    /// Whether the name was written in a case [`local_name`](Self::local_name)
    /// does not keep: when false, `local_name` is the `localName`.
    fn has_written_name(self) -> bool;
    /// The DOM's ID attribute: the no-namespace `id`.
    fn id_attr(self) -> Option<Self::Attr>;
    /// The DOM's class attribute: the no-namespace `class`.
    fn class_attr(self) -> Option<Self::Attr>;
    /// The attribute DOM `getAttribute(qname)` finds, `name` being `qname`
    /// resolved in this element's document.
    fn attr_by_resolved_name(
        self,
        qname: &[u8],
        name: <Self::Host as Host>::AttrName,
    ) -> Option<Self::Attr>;
    /// DOM `getAttribute`.
    fn get_attribute(self, qname: &[u8]) -> Option<&'d [u8]>;
    /// DOM `hasAttribute`.
    fn has_attribute(self, qname: &[u8]) -> bool;
    fn attrs(self) -> Self::Attrs;
}

/// An attribute.
pub trait CssAttr<'d>: Copy {
    fn value(self) -> &'d [u8];
    /// The local name as the host stores it (lower-cased in HTML).
    fn local_name(self) -> &'d [u8];
    /// The DOM's `localName`.
    fn dom_local_name(self) -> &'d [u8];
    /// The attribute's own namespace URI; `None` for no namespace.
    fn ns_uri(self) -> Option<&'d [u8]>;
}

/// A document.
pub trait CssDoc<'d>: Copy {
    type Host: Host;

    /// Whether the document is in quirks mode, where `#id` and `.class`
    /// compare ASCII case-insensitively.
    fn is_quirks(self) -> bool;
    /// `name`'s tag id, folding ASCII case; `None` when no element of the
    /// document can have it.
    fn tag_id(self, name: &[u8]) -> Option<<Self::Host as Host>::TagId>;
    fn resolve_attr_name(self, name: &[u8]) -> <Self::Host as Host>::AttrName;
}
