//! The XPath engine's HTML backend: `Dom` for a Lexbor document, through
//! [`crate::dom_adapter::html`], which is the only module that reads Lexbor's
//! structs.
//!
//! The adapter's typed handles carry the contract - a live node of a document
//! that is not restructured while they are held - so the readers here are safe.
//! What they add is the kind check the raw readers leave to their callers: an
//! element-only or attribute-only read of another kind of node answers empty
//! rather than reading it as the wrong struct.

use core::ffi::c_void;

use super::abi::*;
use super::dom::*;
use crate::dom_adapter::html::{self as dom, HtmlAttr, HtmlDoc, HtmlNode};
use crate::lexbor_abi::{self as lxb, LxbNode};

/* The engine reads every node's type through the shared `NTYPE_*` encoding, so
 * Lexbor's enum must agree value for value; a mismatch would make an HTML walk
 * misread each node rather than fail. Checked at compile time, against the
 * generated header view, in every build that has an HTML backend. */
const _: () = {
    assert!(NTYPE_ELEMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT);
    assert!(NTYPE_ATTRIBUTE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ATTRIBUTE);
    assert!(NTYPE_TEXT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT);
    assert!(NTYPE_CDATA_SECTION == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION);
    assert!(NTYPE_ENTITY_REFERENCE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ENTITY_REFERENCE);
    assert!(NTYPE_ENTITY == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ENTITY);
    assert!(NTYPE_PI == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION);
    assert!(NTYPE_COMMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_COMMENT);
    assert!(NTYPE_DOCUMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT);
    assert!(NTYPE_DOCUMENT_TYPE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE);
    assert!(NTYPE_NOTATION == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_NOTATION);
};

impl<'d> Dom<'d> for HtmlDoc<'d> {
    const IS_XML: bool = false;

    type Node = HtmlNode<'d>;
    type Attr = HtmlAttr<'d>;

    #[inline]
    fn token(n: HtmlNode<'d>) -> *mut c_void {
        n.as_raw() as *mut c_void
    }
    #[inline]
    unsafe fn node(self, p: *mut c_void) -> HtmlNode<'d> {
        debug_assert!(!p.is_null());
        HtmlNode::from_raw(p as *mut LxbNode).unwrap_unchecked()
    }

    #[inline]
    fn document_node(self) -> HtmlNode<'d> {
        self.as_node()
    }
    #[inline]
    fn node_type(self, n: HtmlNode<'d>) -> u32 {
        n.node_type()
    }

    #[inline]
    fn first_child(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.first_child()
    }
    #[inline]
    fn last_child(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.last_child()
    }
    #[inline]
    fn next(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.next()
    }
    #[inline]
    fn prev(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.prev()
    }
    #[inline]
    fn parent(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.parent()
    }

    #[inline]
    fn first_attr(self, el: HtmlNode<'d>) -> Option<HtmlAttr<'d>> {
        el.element()?.first_attr()
    }
    #[inline]
    fn attr_next(self, a: HtmlAttr<'d>) -> Option<HtmlAttr<'d>> {
        a.next_attr()
    }
    #[inline]
    fn attr_node(a: HtmlAttr<'d>) -> HtmlNode<'d> {
        a.node()
    }
    #[inline]
    fn as_attr(self, n: HtmlNode<'d>) -> Option<HtmlAttr<'d>> {
        n.attr()
    }
    #[inline]
    fn attr_value(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.value()
    }
    #[inline]
    fn get_attribute(self, el: HtmlNode<'d>, name: &[u8]) -> Option<&'d [u8]> {
        el.element()?.get_attribute(name)
    }

    #[inline]
    fn local_name(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.element().map_or(&[], |e| e.local_name())
    }
    #[inline]
    fn attr_local_name(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.local_name()
    }
    #[inline]
    fn qualified_name(self, n: HtmlNode<'d>) -> &'d [u8] {
        // SAFETY: a live node; the reader handles every node kind.
        unsafe { dom::qualified_name(n.as_raw()) }
    }
    #[inline]
    fn attr_qualified_name(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.qualified_name()
    }
    #[inline]
    fn pi_name(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.node_name()
    }

    #[inline]
    fn ns_uri(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.ns_uri().unwrap_or(&[])
    }
    #[inline]
    fn is_foreign_ns(self, n: HtmlNode<'d>) -> bool {
        let ns = n.ns_id();
        ns != dom::NS_HTML && ns != dom::NS_UNDEF
    }
    #[inline]
    fn has_ns(self, n: HtmlNode<'d>) -> bool {
        n.ns_id() != dom::NS_UNDEF
    }
    #[inline]
    fn append_own_text(self, n: HtmlNode<'d>, buf: &mut Buf) -> Result<(), BufError> {
        n.with_text_content(|text| text.map_or(Ok(()), |t| buf.append(t)))
    }

    fn name_bucket(
        self,
        cx: &Context,
        local: &[u8],
        ns_uri: Option<&[u8]>,
        _lax: bool,
    ) -> Option<Bucket<'d, HtmlNode<'d>>> {
        if ns_uri.is_some() {
            return None;
        }
        let Backend::Html { index, .. } = cx.backend() else {
            return None;
        };
        // SAFETY: the context's element index belongs to this document and is
        // dropped only by a mutation, which cannot happen while it is lent.
        let index = unsafe { index.as_ref() }?;
        if index.has_foreign() {
            return None;
        }
        // SAFETY: `self` is a live document.
        let tag = unsafe { dom::tag_id_by_name(self.as_raw(), local) };
        if tag == dom::TAG_UNDEF || tag >= dom::TAG_LAST_ENTRY {
            return None;
        }
        let nodes = index.tag_bucket(tag);
        // SAFETY: `HtmlNode` is a transparent non-null node pointer, and the
        // index holds only live elements of this document, none null.
        let nodes: &'d [HtmlNode<'d>] = unsafe {
            core::slice::from_raw_parts(nodes.as_ptr() as *const HtmlNode<'d>, nodes.len())
        };
        Some(Bucket {
            nodes,
            recheck: true,
        })
    }
}
