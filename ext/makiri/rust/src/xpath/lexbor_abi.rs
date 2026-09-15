//! Lexbor's node / element / attr structs as the HTML backend reads them, and
//! the Lexbor entry points it calls.
//!
//! Unlike `mkr_xml_node_t`, these belong to a vendored dependency whose pin
//! moves (CLAUDE.md), so a reordered field would not fail to build - it would
//! read the wrong offset. `lexbor_abi::agree` guards that: it compares every
//! field this file declares against the generated header view at COMPILE time,
//! in every configuration. It also holds the two facts the retired C checker
//! did (that `node` sits first in the element and attr structs, which is what
//! makes the engine's handle casts sound).
//!
//! Only the fields the engine navigates by are declared. Everything else Lexbor
//! offers goes through its exported functions - including the two it publishes
//! as `lxb_inline`, which it also exports as `_noi` for exactly this case.

use super::abi::*;
use super::dom::{Bucket, DomHandle, DomRaw};
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

/// `lxb_dom_node_t`. 96 bytes; the first field is an event-target pointer.
#[repr(C)]
pub struct Node {
    pub events: *mut c_void,
    /// interned, lowercase, without prefix
    pub local_name: usize,
    pub prefix: usize,
    /// namespace id; 0 is LXB_NS__UNDEF
    pub ns: usize,
    pub owner_document: *mut Document,
    pub next: *mut Node,
    pub prev: *mut Node,
    pub parent: *mut Node,
    pub first_child: *mut Node,
    pub last_child: *mut Node,
    /// Reserved by Makiri for a source-location byte offset - do not read it
    /// here, and never write it.
    pub user: *mut c_void,
    pub type_: u32,
}

/// `lxb_dom_element_t`. The engine only navigates to `first_attr`; the fields
/// before it are declared to place it, not to be read.
#[repr(C)]
pub struct Element {
    pub node: Node,
    pub upper_name: usize,
    pub qualified_name: usize,
    pub is_value: *mut c_void,
    pub first_attr: *mut Attr,
    pub last_attr: *mut Attr,
    pub attr_id: *mut Attr,
    pub attr_class: *mut Attr,
    pub style: *mut c_void,
    pub list: *mut c_void,
    /* Two C enums, so 4 bytes each - not pointer-sized like the ids above. */
    pub condition: u32,
    pub custom_state: u32,
}

/// `lxb_dom_attr_t`. Attributes hang off an element in their own list, so the
/// attribute axis walks `next` here rather than the node's.
#[repr(C)]
pub struct Attr {
    pub node: Node,
    pub upper_name: usize,
    pub qualified_name: usize,
    pub value: *mut c_void,
    pub owner: *mut Element,
    pub next: *mut Attr,
    pub prev: *mut Attr,
}

/// `lxb_dom_document_t`, opaque: the engine reaches into it only through the
/// two shims, which see the real header.
#[repr(C)]
pub struct Document {
    _private: [u8; 0],
}

/* ---- the Lexbor constants the engine compares against ----
 *
 * Generated, not restated. These were hand-written with a const-assert against
 * the header, on the reasoning that the hot comparisons should stay immediate
 * values - which a generated `pub const` also is, so the reasoning was simply
 * wrong and the assert was guarding a copy that need not have existed.
 *
 * The incident it was guarding against is real and worth remembering:
 * LXB_NS_HTML is 2, and a hand-written 1 made every HTML element foreign, so
 * every unprefixed name test matched nothing - silently. Deriving the value
 * removes the class rather than checking for it. */
pub const NS_UNDEF: usize = crate::lexbor_abi::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
pub const NS_HTML: usize = crate::lexbor_abi::lxb_ns_id_enum_t_LXB_NS_HTML as usize;

/// `LXB_TAG__UNDEF`. A custom element's tag id is a pointer value, far above
/// the static range the index buckets, so it is compared against
/// `TAG_LAST_ENTRY` rather than this.
pub const TAG_UNDEF: usize = crate::lexbor_abi::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize;

/* Lexbor's exported accessors, re-exported from the generated bindings rather
 * than declared again here.
 *
 * They WERE declared here, over the hand-written structs above, until
 * `glue/html_node` needed the same six and bindgen started emitting them: the
 * same C symbol then had two Rust types, which rustc reports as
 * "redeclared with a different signature". The structs stay hand-written for
 * the reason in `lexbor_abi::agree` - the engine reads their fields per node -
 * but a FUNCTION has no hot path to shape, so there is no reason for a second
 * declaration of one. The handles are cast at the call site, which is what
 * those call sites already did. */
pub use crate::lexbor_abi::{
    lxb_dom_attr_local_name, lxb_dom_attr_qualified_name, lxb_dom_element_get_attribute,
    lxb_dom_element_local_name, lxb_dom_element_qualified_name, lxb_dom_node_name, LxbAttr,
    LxbElement, LxbNode,
};

/// The `_noi` twin Lexbor publishes so a non-C caller can reach an
/// `lxb_inline` function. Declared once, in `lexbor_abi` - see the note there
/// for why that one is hand-written where the rest are generated.
pub use crate::lexbor_abi::lxb_dom_attr_value_noi;

/* ---- the shims, standing alone ----
 *
 * The three above stayed in C for two stated reasons: two of them reach through
 * `lxb_dom_document_t` for a field, and that struct is large, not ours, and
 * moves with the Lexbor pin - so hand-writing a view of it would have been the
 * most fragile part of the port; and the text append owns a Lexbor allocation
 * for the length of the call, which is clearest with the malloc and the free in
 * one function.
 *
 * Only the first reason was ever about C. bindgen generates `lxb_dom_document_t`
 * from Lexbor's own headers, so reading `->ns` and `->tags` here transcribes
 * nothing and moves with the pin exactly as the C did. The second reason is not
 * about language at all: the append and the free still live together, below.
 *
 * `mkr_html_tag_last_entry` disappears rather than moves - it existed to carry a
 * generated constant across the language boundary, and there is no boundary
 * left to carry it across. */

/// `LXB_TAG__LAST_ENTRY` - the end of Lexbor's static tag-id range. Derived from
/// the generated enum, so it moves with the Lexbor pin.
pub const TAG_LAST_ENTRY: usize = crate::lexbor_abi::lxb_tag_id_enum_t_LXB_TAG__LAST_ENTRY as usize;

/// Borrowed namespace-URI bytes for a node, or NULL with `*len` 0 when it has
/// none.
///
/// # Safety
/// `node` and `doc` are NULL or live; `len` is writable.
pub unsafe extern "C" fn mkr_html_ns_uri(
    node: *const Node,
    doc: *const Document,
    len: *mut usize,
) -> *const c_char {
    *len = 0;
    if node.is_null() || (*node).ns == NS_UNDEF || doc.is_null() {
        return core::ptr::null();
    }
    let doc = doc as *const crate::lexbor_abi::LxbDoc;
    if (*doc).ns.is_null() {
        return core::ptr::null();
    }
    crate::lexbor_abi::lxb_ns_by_id((*doc).ns, (*node).ns, len) as *const c_char
}

/// Resolve a tag name to a Lexbor tag id for the `//tag` index fast path, or
/// `LXB_TAG__UNDEF`.
///
/// # Safety
/// `doc` is NULL or live; `p` is NULL or names `len` readable bytes.
pub unsafe extern "C" fn mkr_html_tag_id_by_name(
    doc: *const Document,
    p: *const c_char,
    len: usize,
) -> usize {
    if doc.is_null() || p.is_null() || len == 0 {
        return TAG_UNDEF;
    }
    let doc = doc as *const crate::lexbor_abi::LxbDoc;
    if (*doc).tags.is_null() {
        return TAG_UNDEF;
    }
    crate::lexbor_abi::lxb_tag_id_by_name_noi((*doc).tags, p as *const u8, len)
}

/// Append a node's own text content to `buf`, returning an `mkr_status_t`.
///
/// Lexbor builds the content on demand and hands back an allocation, so the
/// append and the free stay together: the append copies, then the allocation
/// goes back, on every path.
///
/// # Safety
/// `node` is a live node; `buf` is a live buffer.
pub unsafe extern "C" fn mkr_html_append_own_text(node: *mut Node, buf: *mut Buf) -> c_int {
    let mut tlen: usize = 0;
    let t = crate::lexbor_abi::lxb_dom_node_text_content(
        node as *mut crate::lexbor_abi::LxbNode,
        &mut tlen,
    );
    if t.is_null() {
        return crate::xpath_abi::MKR_OK;
    }
    let st = crate::cbuf::mkr_buf_append(buf, t as *const c_void, tlen);
    crate::lexbor_abi::lxb_dom_document_destroy_text_noi(
        (*node).owner_document as *mut crate::lexbor_abi::LxbDoc,
        t,
    );
    st
}

/* ---------- typed views over the Lexbor layout ---------- */

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

/// The document's root node handle.
///
/// # Safety
/// `doc` must be a live Lexbor document.
#[inline]
pub unsafe fn document_node(doc: *mut Document) -> *mut Node {
    doc as *mut Node
}

/// A node's type (`lxb_dom_node_type_t`).
///
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn node_type(node: *mut Node) -> u32 {
    (*node).type_
}
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn first_child(node: *mut Node) -> *mut Node {
    (*node).first_child
}
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn last_child(node: *mut Node) -> *mut Node {
    (*node).last_child
}
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn next(node: *mut Node) -> *mut Node {
    (*node).next
}
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn prev(node: *mut Node) -> *mut Node {
    (*node).prev
}
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn parent(node: *mut Node) -> *mut Node {
    (*node).parent
}

/// # Safety
/// `element` must be a live Lexbor element node.
#[inline]
pub unsafe fn first_attr(element: *mut Node) -> *mut Node {
    (*(element as *mut Element)).first_attr as *mut Node
}
/// # Safety
/// `attr` must be a live Lexbor attribute node.
#[inline]
pub unsafe fn attr_next(attr: *mut Node) -> *mut Node {
    (*(attr as *mut Attr)).next as *mut Node
}
/// An attribute's value, borrowed from the document.
///
/// # Safety
/// `attr` must be a live Lexbor attribute node; the bytes are valid while the
/// document is.
#[inline]
pub unsafe fn attr_value<'a>(attr: *mut Node) -> &'a [u8] {
    named_mut(attr as *mut LxbAttr, lxb_dom_attr_value_noi)
}

/// # Safety
/// `element` must be a live Lexbor element node; the bytes are valid while the
/// document is.
#[inline]
pub unsafe fn get_attribute<'a>(element: *mut Node, name: &[u8]) -> Option<&'a [u8]> {
    let mut len = 0;
    let value = lxb_dom_element_get_attribute(
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

/// # Safety
/// `node` must be a live Lexbor element node; the bytes are valid while the
/// document is.
#[inline]
pub unsafe fn local_name<'a>(node: *mut Node) -> &'a [u8] {
    named_mut(node as *mut LxbElement, lxb_dom_element_local_name)
}
/// # Safety
/// `attr` must be a live Lexbor attribute node; the bytes are valid while the
/// document is.
#[inline]
pub unsafe fn attr_local_name<'a>(attr: *mut Node) -> &'a [u8] {
    named(attr as *mut LxbAttr, lxb_dom_attr_local_name)
}
/// # Safety
/// `node` must be a live Lexbor node; the bytes are valid while the document is.
#[inline]
pub unsafe fn qualified_name<'a>(node: *mut Node) -> &'a [u8] {
    if (*node).type_ == 1 {
        named(node as *mut LxbElement, lxb_dom_element_qualified_name)
    } else {
        named_mut(node as *mut LxbNode, lxb_dom_node_name)
    }
}
/// # Safety
/// `attr` must be a live Lexbor attribute node; the bytes are valid while the
/// document is.
#[inline]
pub unsafe fn attr_qualified_name<'a>(attr: *mut Node) -> &'a [u8] {
    named(attr as *mut LxbAttr, lxb_dom_attr_qualified_name)
}
/// # Safety
/// `node` must be a live Lexbor node; the bytes are valid while the document is.
#[inline]
pub unsafe fn pi_name<'a>(node: *mut Node) -> &'a [u8] {
    named_mut(node as *mut LxbNode, lxb_dom_node_name)
}
/// # Safety
/// `node` must be a live Lexbor node and `doc` its live document.
#[inline]
pub unsafe fn ns_uri<'a>(node: *mut Node, doc: *mut Document) -> &'a [u8] {
    let mut len = 0;
    seen(mkr_html_ns_uri(node, doc, &mut len) as *const u8, len)
}
/// # Safety
/// `node` must be a live Lexbor element node.
#[inline]
pub unsafe fn is_foreign_ns(node: *mut Node) -> bool {
    (*node).ns != NS_HTML && (*node).ns != NS_UNDEF
}
/// # Safety
/// `node` must be a live Lexbor node.
#[inline]
pub unsafe fn has_ns(node: *mut Node) -> bool {
    (*node).ns != NS_UNDEF
}

/// # Safety
/// `doc` must be a live Lexbor document.
#[inline]
pub unsafe fn tag_id_by_name(doc: *const Document, local: &[u8]) -> usize {
    mkr_html_tag_id_by_name(doc, local.as_ptr() as *const c_char, local.len())
}
/// # Safety
/// `nodes` must name `count` live handles (or be null when `count == 0`), and
/// they must stay valid for `'a`.
#[inline]
pub unsafe fn bucket<'a>(nodes: *const *mut c_void, count: usize) -> &'a [*mut c_void] {
    if nodes.is_null() || count == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(nodes, count)
    }
}

/* ---------- the HTML backend's raw adapter ---------- */

/// Lexbor-backed XPath representation.
pub struct Html;

unsafe impl DomHandle for Html {
    type Node = *mut Node;
    type Doc = *mut Document;

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
        document_node(doc)
    }
    unsafe fn raw_node_type(_doc: Self::Doc, n: Self::Node) -> u32 {
        node_type(n)
    }
    unsafe fn raw_first_child(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        first_child(n)
    }
    unsafe fn raw_last_child(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        last_child(n)
    }
    unsafe fn raw_next(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        next(n)
    }
    unsafe fn raw_prev(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        prev(n)
    }
    unsafe fn raw_parent(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        parent(n)
    }
    unsafe fn raw_first_attr(_doc: Self::Doc, el: Self::Node) -> Self::Node {
        first_attr(el)
    }
    unsafe fn raw_attr_next(_doc: Self::Doc, a: Self::Node) -> Self::Node {
        attr_next(a)
    }
    unsafe fn raw_attr_value<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        attr_value(a)
    }
    unsafe fn raw_get_attribute<'a>(
        _doc: Self::Doc,
        el: Self::Node,
        name: &[u8],
    ) -> Option<&'a [u8]> {
        get_attribute(el, name)
    }
    unsafe fn raw_local_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        local_name(n)
    }
    unsafe fn raw_attr_local_name<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        attr_local_name(a)
    }
    unsafe fn raw_qualified_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        qualified_name(n)
    }
    unsafe fn raw_attr_qualified_name<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        attr_qualified_name(a)
    }
    unsafe fn raw_pi_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        pi_name(n)
    }
    unsafe fn raw_ns_uri<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        ns_uri(n, doc)
    }
    unsafe fn raw_is_foreign_ns(_doc: Self::Doc, n: Self::Node) -> bool {
        is_foreign_ns(n)
    }
    unsafe fn raw_has_ns(_doc: Self::Doc, n: Self::Node) -> bool {
        has_ns(n)
    }
    unsafe fn raw_append_own_text(_doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int {
        mkr_html_append_own_text(n, buf)
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
        let index = mkr_ctx_element_index(ctx);
        let lookup = mkr_ctx_tag_lookup(ctx)?;
        let has_foreign = mkr_ctx_tag_has_foreign(ctx)?;
        if index.is_null() || has_foreign(index) != 0 {
            return None;
        }
        let doc = mkr_ctx_document(ctx) as *const Document;
        if doc.is_null() {
            return None;
        }
        let tag = tag_id_by_name(doc, local);
        if tag == TAG_UNDEF || tag >= TAG_LAST_ENTRY {
            return None;
        }
        let mut cnt = 0usize;
        let nodes = bucket(lookup(index, tag, &mut cnt), cnt);
        Some(Bucket {
            nodes,
            recheck: true,
        })
    }
}
