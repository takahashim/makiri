//! The arena's handles as the CSS matcher reads them (`css_match::dom`):
//! every item forwards to the handle's own method of the same name.

use crate::css_match::dom::{CssAttr, CssDoc, CssElement, CssNode, Host, NsFilter};

use super::handle::{AttrName, Attrs, HtmlAttr, HtmlDoc, HtmlElement, HtmlNode, NsId, TagId};
use super::NodeType;

/// The HTML arena as a [`Host`] of the matcher.
pub struct HtmlHost;

impl Host for HtmlHost {
    const RESOLVES_NAMES: bool = true;
    type Node<'d> = HtmlNode<'d>;
    type TagId = TagId;
    type AttrName = AttrName;
}

impl<'d> CssNode<'d> for HtmlNode<'d> {
    type Host = HtmlHost;
    type Element = HtmlElement<'d>;
    type Doc = HtmlDoc<'d>;

    #[inline]
    fn element(self) -> Option<HtmlElement<'d>> {
        HtmlNode::element(self)
    }
    #[inline]
    fn parent(self) -> Option<Self> {
        HtmlNode::parent(self)
    }
    #[inline]
    fn first_child(self) -> Option<Self> {
        HtmlNode::first_child(self)
    }
    #[inline]
    fn next(self) -> Option<Self> {
        HtmlNode::next(self)
    }
    #[inline]
    fn prev(self) -> Option<Self> {
        HtmlNode::prev(self)
    }
    #[inline]
    fn preorder_next(self, root: Self) -> Option<Self> {
        HtmlNode::preorder_next(self, root)
    }
    #[inline]
    fn preorder_next_past(self, root: Self) -> Option<Self> {
        HtmlNode::preorder_next_past(self, root)
    }
    #[inline]
    fn node_type(self) -> NodeType {
        HtmlNode::node_type(self)
    }
    #[inline]
    fn char_data(self) -> Option<&'d [u8]> {
        HtmlNode::char_data(self)
    }
    #[inline]
    fn is_html(self) -> bool {
        self.ns_id() == Some(NsId::HTML)
    }
    /// Every document here is an HTML document.
    #[inline]
    fn folds_case(self) -> bool {
        CssNode::is_html(self)
    }
    #[inline]
    fn ns_uri(self) -> Option<&'d [u8]> {
        HtmlNode::ns_uri(self)
    }
    #[inline]
    fn same_namespace(self, other: Self) -> bool {
        self.ns_id() == other.ns_id()
    }
    #[inline]
    fn tag_id(self) -> Option<TagId> {
        HtmlNode::tag_id(self)
    }
    #[inline]
    fn owner_document(self) -> HtmlDoc<'d> {
        HtmlNode::owner_document(self)
    }
    fn is_root(self) -> bool {
        self.owner_document().as_node().document_root() == Some(self)
    }
    #[inline]
    fn table_key(self) -> crate::ptr_table::NodeKey {
        super::RawNode::from(self).table_key()
    }

    /// The walk, with its tag-id filter (`css_match::query`), is the HTML
    /// matcher's: no index is offered.
    type Candidates = core::iter::Empty<Self>;
    #[inline]
    fn candidates(self, _local: &[u8], _ns: NsFilter<'_>) -> Option<Self::Candidates> {
        None
    }
}

impl<'d> CssElement<'d> for HtmlElement<'d> {
    type Node = HtmlNode<'d>;
    type Host = HtmlHost;
    type Attr = HtmlAttr<'d>;
    type Attrs = Attrs<'d>;

    #[inline]
    fn node(self) -> HtmlNode<'d> {
        HtmlElement::node(self)
    }
    #[inline]
    fn local_name(self) -> &'d [u8] {
        HtmlElement::local_name(self)
    }
    #[inline]
    fn dom_local_name(self) -> &'d [u8] {
        HtmlElement::dom_local_name(self)
    }
    #[inline]
    fn has_written_name(self) -> bool {
        HtmlElement::has_written_name(self)
    }
    #[inline]
    fn id_attr(self) -> Option<HtmlAttr<'d>> {
        HtmlElement::id_attr(self)
    }
    #[inline]
    fn class_attr(self) -> Option<HtmlAttr<'d>> {
        HtmlElement::class_attr(self)
    }
    #[inline]
    fn attr_by_resolved_name(self, qname: &[u8], name: AttrName) -> Option<HtmlAttr<'d>> {
        HtmlElement::attr_by_resolved_name(self, qname, name)
    }
    #[inline]
    fn get_attribute(self, qname: &[u8]) -> Option<&'d [u8]> {
        HtmlElement::get_attribute(self, qname)
    }
    #[inline]
    fn has_attribute(self, qname: &[u8]) -> bool {
        HtmlElement::has_attribute(self, qname)
    }
    #[inline]
    fn attrs(self) -> Attrs<'d> {
        HtmlElement::attrs(self)
    }
}

impl<'d> CssAttr<'d> for HtmlAttr<'d> {
    #[inline]
    fn value(self) -> &'d [u8] {
        HtmlAttr::value(self)
    }
    #[inline]
    fn local_name(self) -> &'d [u8] {
        HtmlAttr::local_name(self)
    }
    #[inline]
    fn dom_local_name(self) -> &'d [u8] {
        HtmlAttr::dom_local_name(self)
    }
    #[inline]
    fn ns_uri(self) -> Option<&'d [u8]> {
        HtmlAttr::own_ns_uri(self)
    }
}

impl<'d> CssDoc<'d> for HtmlDoc<'d> {
    type Host = HtmlHost;

    /// Lexbor's `compat_mode` values: 0 no-quirks, 1 quirks, 2 limited-quirks.
    #[inline]
    fn is_quirks(self) -> bool {
        self.compat_mode() == 1
    }
    #[inline]
    fn tag_id(self, name: &[u8]) -> Option<TagId> {
        HtmlDoc::tag_id(self, name)
    }
    #[inline]
    fn resolve_attr_name(self, name: &[u8]) -> AttrName {
        HtmlDoc::resolve_attr_name(self, name)
    }
}
