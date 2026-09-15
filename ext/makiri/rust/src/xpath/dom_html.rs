//! The XPath engine's HTML backend: `Html` binds the generic engine to Lexbor's
//! DOM through [`crate::dom_adapter::html`], which is the only module that reads
//! Lexbor's structs.

use core::ffi::{c_int, c_void};
use core::ptr;

use super::abi::*;
use super::dom::*;
use crate::dom_adapter::html as dom;
use crate::lexbor_abi::{self as lxb, LxbDoc, LxbNode};

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

/// Lexbor-backed XPath representation.
pub struct Html;

unsafe impl DomHandle for Html {
    type Node = *mut LxbNode;
    type Doc = *mut LxbDoc;

    fn null() -> Self::Node {
        ptr::null_mut()
    }
    fn is_null(n: Self::Node) -> bool {
        n.is_null()
    }
    fn to_void(n: Self::Node) -> *mut c_void {
        n as *mut c_void
    }
    unsafe fn from_void(p: *mut c_void) -> Self::Node {
        p as Self::Node
    }
    unsafe fn doc_from_void(p: *mut c_void) -> Self::Doc {
        p as Self::Doc
    }
}

unsafe impl DomRaw for Html {
    const RAW_IS_XML: bool = false;

    unsafe fn raw_document_node(doc: Self::Doc) -> Self::Node {
        dom::document_node(doc)
    }
    unsafe fn raw_node_type(_doc: Self::Doc, n: Self::Node) -> u32 {
        dom::node_type(n)
    }
    unsafe fn raw_first_child(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        dom::first_child(n)
    }
    unsafe fn raw_last_child(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        dom::last_child(n)
    }
    unsafe fn raw_next(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        dom::next(n)
    }
    unsafe fn raw_prev(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        dom::prev(n)
    }
    unsafe fn raw_parent(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        dom::parent(n)
    }
    unsafe fn raw_first_attr(_doc: Self::Doc, el: Self::Node) -> Self::Node {
        dom::first_attr(el)
    }
    unsafe fn raw_attr_next(_doc: Self::Doc, a: Self::Node) -> Self::Node {
        dom::attr_next(a)
    }
    unsafe fn raw_attr_value<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        dom::attr_value(a)
    }
    unsafe fn raw_get_attribute<'a>(
        _doc: Self::Doc,
        el: Self::Node,
        name: &[u8],
    ) -> Option<&'a [u8]> {
        dom::get_attribute(el, name)
    }
    unsafe fn raw_local_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        dom::local_name(n)
    }
    unsafe fn raw_attr_local_name<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        dom::attr_local_name(a)
    }
    unsafe fn raw_qualified_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        dom::qualified_name(n)
    }
    unsafe fn raw_attr_qualified_name<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        dom::attr_qualified_name(a)
    }
    unsafe fn raw_pi_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        dom::pi_name(n)
    }
    unsafe fn raw_ns_uri<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        dom::ns_uri(n)
    }
    unsafe fn raw_is_foreign_ns(_doc: Self::Doc, n: Self::Node) -> bool {
        dom::is_foreign_ns(n)
    }
    unsafe fn raw_has_ns(_doc: Self::Doc, n: Self::Node) -> bool {
        dom::has_ns(n)
    }
    unsafe fn raw_append_own_text(_doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int {
        dom::append_own_text(n, buf)
    }

    unsafe fn raw_name_bucket<'a>(
        ctx: *mut Context,
        local: &[u8],
        ns_uri: Option<&[u8]>,
        _lax: bool,
    ) -> Option<Bucket<'a>> {
        if ns_uri.is_some() {
            return None;
        }
        let Some(Backend::Html { index }) = ctx_backend(ctx) else {
            return None;
        };
        if index.is_null() || crate::dom_adapter::dom_index::element_index_has_foreign(index) != 0 {
            return None;
        }
        let tag = dom::tag_id_by_name(ctx_document(ctx) as *const LxbDoc, local);
        if tag == dom::TAG_UNDEF || tag >= dom::TAG_LAST_ENTRY {
            return None;
        }
        let mut cnt = 0usize;
        let nodes = crate::dom_adapter::dom_index::element_index_tag(index, tag, &mut cnt)
            as *const *mut c_void;
        let nodes: &'a [*mut c_void] = if nodes.is_null() || cnt == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(nodes, cnt)
        };
        Some(Bucket {
            nodes,
            recheck: true,
        })
    }
}
