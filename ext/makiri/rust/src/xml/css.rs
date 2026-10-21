//! The XML arena as the CSS matcher reads it (`css_match::dom`): a node is
//! its document and its [`NodeId`], as the HTML arena's handles are.
//!
//! What the HTML arena answers from ids, this answers from bytes: there are
//! no tag ids ([`XmlHost`] resolves no names, so the matcher compares a
//! selector's names as written), and no `id` / `class` shortcut (the
//! attribute is found by scanning). An XML document is no HTML document:
//! nothing folds case, an XHTML element included, and there is no quirks
//! mode. A namespace declaration is no attribute here, as on XPath's
//! attribute axis.

#![forbid(unsafe_code)]

use crate::css_match::dom::{CssAttr, CssDoc, CssElement, CssNode, Host};
use crate::node_type::NodeType;
use crate::xml::model::{ArenaKind, Document, NodeId};

/// The XHTML namespace, whose elements the form-state pseudo-classes know.
const XHTML_NS: &[u8] = b"http://www.w3.org/1999/xhtml";

/// The XML arena as a [`Host`] of the matcher.
pub struct XmlHost;

impl Host for XmlHost {
    const RESOLVES_NAMES: bool = false;
    type Node<'d> = XmlNode<'d>;
    type TagId = ();
    type AttrName = ();
}

/// A node of an XML document, borrowed for `'d`.
#[derive(Clone, Copy)]
pub struct XmlNode<'d> {
    d: &'d Document,
    id: NodeId,
}

impl PartialEq for XmlNode<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && core::ptr::eq(self.d, other.d)
    }
}

impl<'d> XmlNode<'d> {
    pub fn new(d: &'d Document, id: NodeId) -> Self {
        XmlNode { d, id }
    }

    pub fn id(self) -> NodeId {
        self.id
    }

    #[inline]
    fn at(self, id: Option<NodeId>) -> Option<Self> {
        id.map(|id| XmlNode { d: self.d, id })
    }

    #[inline]
    fn kind(self) -> Option<ArenaKind> {
        self.d.type_(self.id)
    }
}

/// An element.
#[derive(Clone, Copy)]
pub struct XmlElement<'d>(XmlNode<'d>);

/// An attribute - never a namespace declaration.
#[derive(Clone, Copy)]
pub struct XmlAttr<'d>(XmlNode<'d>);

/// An element's attributes, namespace declarations left out.
pub struct XmlAttrs<'d> {
    d: &'d Document,
    next: Option<NodeId>,
}

impl<'d> Iterator for XmlAttrs<'d> {
    type Item = XmlAttr<'d>;

    fn next(&mut self) -> Option<XmlAttr<'d>> {
        while let Some(a) = self.next {
            self.next = self.d.next(a);
            if self.d.decl_prefix(a).is_none() {
                return Some(XmlAttr(XmlNode { d: self.d, id: a }));
            }
        }
        None
    }
}

/// A document: nothing the matcher asks of one depends on which.
#[derive(Clone, Copy)]
pub struct XmlDoc<'d>(core::marker::PhantomData<&'d Document>);

impl<'d> CssNode<'d> for XmlNode<'d> {
    type Host = XmlHost;
    type Element = XmlElement<'d>;
    type Doc = XmlDoc<'d>;

    #[inline]
    fn element(self) -> Option<XmlElement<'d>> {
        (self.kind() == Some(ArenaKind::Element)).then_some(XmlElement(self))
    }
    #[inline]
    fn parent(self) -> Option<Self> {
        self.at(self.d.parent(self.id))
    }
    #[inline]
    fn first_child(self) -> Option<Self> {
        self.at(self.d.first_child(self.id))
    }
    #[inline]
    fn next(self) -> Option<Self> {
        self.at(self.d.next(self.id))
    }
    #[inline]
    fn prev(self) -> Option<Self> {
        self.at(self.d.prev(self.id))
    }
    #[inline]
    fn preorder_next(self, root: Self) -> Option<Self> {
        self.at(self.d.preorder_next(root.id, self.id))
    }
    fn preorder_next_past(self, root: Self) -> Option<Self> {
        let mut at = self;
        loop {
            if at == root {
                return None;
            }
            if let Some(next) = at.next() {
                return Some(next);
            }
            at = at.parent()?;
        }
    }
    #[inline]
    fn node_type(self) -> NodeType {
        self.kind().map_or(NodeType::Other, NodeType::from)
    }
    fn char_data(self) -> Option<&'d [u8]> {
        matches!(
            self.kind(),
            Some(ArenaKind::Text | ArenaKind::CDataSection | ArenaKind::Comment)
        )
        .then(|| self.d.value(self.id))
    }
    #[inline]
    fn is_html(self) -> bool {
        self.d.ns(self.id) == XHTML_NS
    }
    /// No XML document is an HTML document.
    #[inline]
    fn folds_case(self) -> bool {
        false
    }
    #[inline]
    fn ns_uri(self) -> Option<&'d [u8]> {
        let uri = self.d.ns(self.id);
        (!uri.is_empty()).then_some(uri)
    }
    #[inline]
    fn same_namespace(self, other: Self) -> bool {
        self.d.ns(self.id) == other.d.ns(other.id)
    }
    /// Never asked: [`XmlHost`] resolves no names.
    #[inline]
    fn tag_id(self) -> Option<()> {
        None
    }
    #[inline]
    fn owner_document(self) -> XmlDoc<'d> {
        XmlDoc(core::marker::PhantomData)
    }
    #[inline]
    fn is_root(self) -> bool {
        self.d.root() == Some(self.id)
    }
    /// The id's token: never 0 (no id names slot 0), and unique in the one
    /// document a query walks.
    #[inline]
    fn table_key(self) -> *const () {
        core::ptr::without_provenance(self.id.to_token())
    }
}

impl<'d> XmlElement<'d> {
    /// The attribute in no namespace whose local name is `local`.
    fn plain_attr(self, local: &[u8]) -> Option<XmlAttr<'d>> {
        self.attrs()
            .find(|a| a.0.d.ns(a.0.id).is_empty() && a.0.d.local(a.0.id) == local)
    }
}

impl<'d> CssElement<'d> for XmlElement<'d> {
    type Node = XmlNode<'d>;
    type Host = XmlHost;
    type Attr = XmlAttr<'d>;
    type Attrs = XmlAttrs<'d>;

    #[inline]
    fn node(self) -> XmlNode<'d> {
        self.0
    }
    #[inline]
    fn local_name(self) -> &'d [u8] {
        self.0.d.local(self.0.id)
    }
    #[inline]
    fn dom_local_name(self) -> &'d [u8] {
        self.0.d.local(self.0.id)
    }
    /// An XML name is kept as written, so it is never the lower-cased form
    /// [`CssElement::has_written_name`] contrasts it with.
    #[inline]
    fn has_written_name(self) -> bool {
        true
    }
    fn id_attr(self) -> Option<XmlAttr<'d>> {
        self.plain_attr(b"id")
    }
    fn class_attr(self) -> Option<XmlAttr<'d>> {
        self.plain_attr(b"class")
    }
    /// Never asked: [`XmlHost`] resolves no names.
    fn attr_by_resolved_name(self, qname: &[u8], (): ()) -> Option<XmlAttr<'d>> {
        self.attrs().find(|a| a.0.d.qname(a.0.id) == qname)
    }
    fn get_attribute(self, qname: &[u8]) -> Option<&'d [u8]> {
        self.attrs()
            .find(|a| a.0.d.qname(a.0.id) == qname)
            .map(CssAttr::value)
    }
    fn has_attribute(self, qname: &[u8]) -> bool {
        self.get_attribute(qname).is_some()
    }
    #[inline]
    fn attrs(self) -> XmlAttrs<'d> {
        XmlAttrs {
            d: self.0.d,
            next: self.0.d.first_attr(self.0.id),
        }
    }
}

impl<'d> CssAttr<'d> for XmlAttr<'d> {
    #[inline]
    fn value(self) -> &'d [u8] {
        self.0.d.value(self.0.id)
    }
    #[inline]
    fn local_name(self) -> &'d [u8] {
        self.0.d.local(self.0.id)
    }
    #[inline]
    fn dom_local_name(self) -> &'d [u8] {
        self.0.d.local(self.0.id)
    }
    #[inline]
    fn ns_uri(self) -> Option<&'d [u8]> {
        CssNode::ns_uri(self.0)
    }
}

impl<'d> CssDoc<'d> for XmlDoc<'d> {
    type Host = XmlHost;

    #[inline]
    fn is_quirks(self) -> bool {
        false
    }
    /// Never asked: [`XmlHost`] resolves no names.
    #[inline]
    fn tag_id(self, _name: &[u8]) -> Option<()> {
        None
    }
    #[inline]
    fn resolve_attr_name(self, _name: &[u8]) {}
}
