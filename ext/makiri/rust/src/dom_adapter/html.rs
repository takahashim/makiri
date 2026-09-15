//! The one place Makiri reads Lexbor's DOM: node, element, attribute and
//! document fields, and the Lexbor accessors over them.
//!
//! Everything here reads the GENERATED layout (`crate::lexbor_abi`), so there is
//! no hand-written copy of a Lexbor struct left to drift from the pinned headers.
//! The XPath engine's HTML backend and the Ruby-facing readers both come through
//! this module, so a field is read one way, in one place.
//!
//! The functions take raw handles for now. Each states its contract once: the
//! handle is live, and the document it belongs to is not being changed while a
//! borrowed slice is in use - which `glue::doc::DocumentEvaluation` enforces for
//! the one place Ruby can run mid-read, an XPath handler.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use crate::cbuf::{buf_append, Buf, BUF_OK};
use crate::lexbor_abi::{self as lxb, LxbAttr, LxbDoc, LxbElement, LxbNode};

/* A node handle is cast to an element or attribute handle, which is sound only
 * while the node sits FIRST in both. That is a claim about the absolute offset,
 * so it is asserted as zero: if Lexbor ever put a field ahead of `node`, every
 * such cast would become wrong. */
const _: () = assert!(
    core::mem::offset_of!(lxb::lxb_dom_element_t, node) == 0,
    "lxb_dom_element_t no longer starts with its node - the handle cast is unsound"
);
const _: () = assert!(
    core::mem::offset_of!(lxb::lxb_dom_attr_t, node) == 0,
    "lxb_dom_attr_t no longer starts with its node - the handle cast is unsound"
);

/* ---- the Lexbor constants the readers compare against ----
 *
 * Generated, not restated. LXB_NS_HTML is 2, and a hand-written 1 once made
 * every HTML element foreign, so every unprefixed name test matched nothing -
 * silently. Deriving the value removes the class rather than checking for it. */
pub const NS_UNDEF: usize = lxb::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
pub const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;

/// `LXB_TAG__UNDEF`. A custom element's tag id is a pointer value, far above
/// the static range the element index buckets, so it is compared against
/// [`TAG_LAST_ENTRY`] rather than this.
pub const TAG_UNDEF: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize;

/// `LXB_TAG__LAST_ENTRY` - the end of Lexbor's static tag-id range.
pub const TAG_LAST_ENTRY: usize = lxb::lxb_tag_id_enum_t_LXB_TAG__LAST_ENTRY as usize;

/* ---------- borrowed bytes ---------- */

#[inline]
unsafe fn seen<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p, len)
    }
}

#[inline]
unsafe fn named<'a, T>(
    h: *mut T,
    f: unsafe extern "C" fn(*const T, *mut usize) -> *const u8,
) -> &'a [u8] {
    let mut len = 0;
    seen(f(h, &mut len), len)
}

#[inline]
unsafe fn named_mut<'a, T>(
    h: *mut T,
    f: unsafe extern "C" fn(*mut T, *mut usize) -> *const u8,
) -> &'a [u8] {
    let mut len = 0;
    seen(f(h, &mut len), len)
}

/* ---------- navigation ---------- */

/// The document's root node handle. An `lxb_dom_document_t` leads with its
/// node, so this is a cast to the embedded base.
#[inline]
pub unsafe fn document_node(doc: *mut LxbDoc) -> *mut LxbNode {
    doc as *mut LxbNode
}

/// A node's type (`lxb_dom_node_type_t`).
#[inline]
pub unsafe fn node_type(node: *mut LxbNode) -> u32 {
    (*node).type_
}
#[inline]
pub unsafe fn first_child(node: *mut LxbNode) -> *mut LxbNode {
    (*node).first_child
}
#[inline]
pub unsafe fn last_child(node: *mut LxbNode) -> *mut LxbNode {
    (*node).last_child
}
#[inline]
pub unsafe fn next(node: *mut LxbNode) -> *mut LxbNode {
    (*node).next
}
#[inline]
pub unsafe fn prev(node: *mut LxbNode) -> *mut LxbNode {
    (*node).prev
}
#[inline]
pub unsafe fn parent(node: *mut LxbNode) -> *mut LxbNode {
    (*node).parent
}

/* ---------- attributes ---------- */

/// `element` must be an element node.
#[inline]
pub unsafe fn first_attr(element: *mut LxbNode) -> *mut LxbNode {
    (*(element as *mut LxbElement)).first_attr as *mut LxbNode
}
/// `attr` must be an attribute node.
#[inline]
pub unsafe fn attr_next(attr: *mut LxbNode) -> *mut LxbNode {
    (*(attr as *mut LxbAttr)).next as *mut LxbNode
}
/// An attribute's value, borrowed from the document.
#[inline]
pub unsafe fn attr_value<'a>(attr: *mut LxbNode) -> &'a [u8] {
    named_mut(attr as *mut LxbAttr, lxb::lxb_dom_attr_value_noi)
}
/// The value of `element`'s attribute named `name` (Lexbor's lookup), or None.
#[inline]
pub unsafe fn get_attribute<'a>(element: *mut LxbNode, name: &[u8]) -> Option<&'a [u8]> {
    let mut len = 0;
    let value = lxb::lxb_dom_element_get_attribute(
        element as *mut LxbElement,
        name.as_ptr(),
        name.len(),
        &mut len,
    );
    if value.is_null() {
        None
    } else {
        Some(seen(value, len))
    }
}

/* ---------- names ---------- */

/// `node` must be an element node.
#[inline]
pub unsafe fn local_name<'a>(node: *mut LxbNode) -> &'a [u8] {
    named_mut(node as *mut LxbElement, lxb::lxb_dom_element_local_name)
}
/// `attr` must be an attribute node.
#[inline]
pub unsafe fn attr_local_name<'a>(attr: *mut LxbNode) -> &'a [u8] {
    named(attr as *mut LxbAttr, lxb::lxb_dom_attr_local_name)
}
#[inline]
pub unsafe fn qualified_name<'a>(node: *mut LxbNode) -> &'a [u8] {
    if (*node).type_ == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT {
        named(node as *mut LxbElement, lxb::lxb_dom_element_qualified_name)
    } else {
        named_mut(node, lxb::lxb_dom_node_name)
    }
}
/// `attr` must be an attribute node.
#[inline]
pub unsafe fn attr_qualified_name<'a>(attr: *mut LxbNode) -> &'a [u8] {
    named(attr as *mut LxbAttr, lxb::lxb_dom_attr_qualified_name)
}
#[inline]
pub unsafe fn pi_name<'a>(node: *mut LxbNode) -> &'a [u8] {
    named_mut(node, lxb::lxb_dom_node_name)
}

/* ---------- namespaces ---------- */

/// The node's namespace URI, borrowed from its document's namespace table, or
/// empty when it has none.
pub unsafe fn ns_uri<'a>(node: *mut LxbNode) -> &'a [u8] {
    if node.is_null() || (*node).ns == NS_UNDEF {
        return &[];
    }
    let doc = (*node).owner_document;
    if doc.is_null() || (*doc).ns.is_null() {
        return &[];
    }
    let mut len = 0;
    seen(lxb::lxb_ns_by_id((*doc).ns, (*node).ns, &mut len), len)
}
/// Whether an element is in a namespace other than HTML's.
#[inline]
pub unsafe fn is_foreign_ns(node: *mut LxbNode) -> bool {
    (*node).ns != NS_HTML && (*node).ns != NS_UNDEF
}
#[inline]
pub unsafe fn has_ns(node: *mut LxbNode) -> bool {
    (*node).ns != NS_UNDEF
}

/* ---------- documents ---------- */

/// A tag name as Lexbor's tag id, for the `//tag` index, or [`TAG_UNDEF`].
pub unsafe fn tag_id_by_name(doc: *const LxbDoc, local: &[u8]) -> usize {
    if doc.is_null() || local.is_empty() || (*doc).tags.is_null() {
        return TAG_UNDEF;
    }
    lxb::lxb_tag_id_by_name_noi((*doc).tags, local.as_ptr(), local.len())
}

/* ---------- text ---------- */

/// Append a node's own text content to `buf`, returning a `cbuf` status.
///
/// Lexbor builds the content on demand and hands back an allocation, so the
/// append and the free stay together: the append copies, then the allocation
/// goes back, on every path.
pub unsafe fn append_own_text(node: *mut LxbNode, buf: *mut Buf) -> c_int {
    let mut tlen: usize = 0;
    let t = lxb::lxb_dom_node_text_content(node, &mut tlen);
    if t.is_null() {
        return BUF_OK;
    }
    let st = buf_append(buf, t as *const c_void, tlen);
    lxb::lxb_dom_document_destroy_text_noi((*node).owner_document, t);
    st
}
