//! The HTML binding of the node-access contract (mkr_xpath_node_access_html.h).
//!
//! It differs from XML in three ways the engine has to see: an element and an
//! attribute are distinct structs with the node embedded first (so a handle
//! casts), attributes live on their own list rather than the sibling chain, and
//! a name is interned - the bytes come from a Lexbor accessor, not a field.

/* See dom_xml.rs: the trait states the precondition once. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::dom::{Bucket, Dom, DomHandle, NTYPE_ELEMENT};
use super::html_abi as lxb;
use core::ffi::{c_int, c_void};
use core::ptr;

/// Lexbor's `lxb_dom_node_t`.
pub struct Html;

/// Borrow a (ptr, len) pair Lexbor handed back, empty when it returned NULL.
#[inline]
unsafe fn seen<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p, len)
    }
}

/// Call one of Lexbor's `(handle, *mut len) -> *const u8` accessors. They come
/// in both pointer flavours, so there are two of these rather than the call
/// being spelled out at each site that does not fit.
#[inline]
unsafe fn named<'a, T>(
    h: *mut T,
    f: unsafe extern "C" fn(*const T, *mut usize) -> *const u8,
) -> &'a [u8] {
    let mut len = 0usize;
    seen(f(h, &mut len), len)
}

#[inline]
unsafe fn named_mut<'a, T>(
    h: *mut T,
    f: unsafe extern "C" fn(*mut T, *mut usize) -> *const u8,
) -> &'a [u8] {
    let mut len = 0usize;
    seen(f(h, &mut len), len)
}

unsafe impl DomHandle for Html {
    type Node = *mut lxb::Node;
    type Doc = *mut lxb::Document;

    #[inline]
    fn null() -> Self::Node {
        ptr::null_mut()
    }
    #[inline]
    fn is_null(n: Self::Node) -> bool {
        n.is_null()
    }
    #[inline]
    fn to_void(n: Self::Node) -> *mut c_void {
        n as *mut c_void
    }
    #[inline]
    unsafe fn from_void(p: *mut c_void) -> Self::Node {
        p as Self::Node
    }
    #[inline]
    unsafe fn doc_from_void(p: *mut c_void) -> Self::Doc {
        p as Self::Doc
    }
}

unsafe impl Dom for Html {
    const IS_XML: bool = false;

    #[inline]
    unsafe fn document_node(doc: Self::Doc) -> Self::Node {
        doc as *mut lxb::Node
    }

    #[inline]
    unsafe fn node_type(_doc: Self::Doc, n: Self::Node) -> u32 {
        (*n).type_
    }

    #[inline]
    unsafe fn first_child(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        (*n).first_child
    }
    #[inline]
    unsafe fn last_child(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        (*n).last_child
    }
    #[inline]
    unsafe fn next(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        (*n).next
    }
    #[inline]
    unsafe fn prev(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        (*n).prev
    }
    #[inline]
    unsafe fn parent(_doc: Self::Doc, n: Self::Node) -> Self::Node {
        (*n).parent
    }

    /* An element and an attribute embed the node first, so a handle is the
     * same address either way - that is what the C's lxb_dom_interface_*
     * casts are, and the layout check asserts both offsets are 0. */
    #[inline]
    unsafe fn first_attr(_doc: Self::Doc, el: Self::Node) -> Self::Node {
        (*(el as *mut lxb::Element)).first_attr as Self::Node
    }
    #[inline]
    unsafe fn attr_next(_doc: Self::Doc, a: Self::Node) -> Self::Node {
        (*(a as *mut lxb::Attr)).next as Self::Node
    }
    #[inline]
    unsafe fn attr_value<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        named_mut(a as *mut lxb::LxbAttr, lxb::lxb_dom_attr_value_noi)
    }

    unsafe fn get_attribute<'a>(_doc: Self::Doc, el: Self::Node, name: &[u8]) -> Option<&'a [u8]> {
        let mut len = 0usize;
        let v = lxb::lxb_dom_element_get_attribute(
            el as *mut lxb::LxbElement,
            name.as_ptr(),
            name.len(),
            &mut len,
        );
        if v.is_null() {
            None
        } else {
            Some(seen(v, len))
        }
    }

    #[inline]
    unsafe fn local_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        named_mut(n as *mut lxb::LxbElement, lxb::lxb_dom_element_local_name)
    }
    #[inline]
    unsafe fn attr_local_name<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        named(a as *mut lxb::LxbAttr, lxb::lxb_dom_attr_local_name)
    }

    /// An HTML element reports its lowercase local name, which is the data
    /// model the rest of Makiri assumes (`Node#name`); every other kind
    /// defers to Lexbor's node name.
    unsafe fn qualified_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        if (*n).type_ == NTYPE_ELEMENT {
            named(
                n as *mut lxb::LxbElement,
                lxb::lxb_dom_element_qualified_name,
            )
        } else {
            named_mut(n as *mut lxb::LxbNode, lxb::lxb_dom_node_name)
        }
    }
    #[inline]
    unsafe fn attr_qualified_name<'a>(_doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        named(a as *mut lxb::LxbAttr, lxb::lxb_dom_attr_qualified_name)
    }
    #[inline]
    unsafe fn pi_name<'a>(_doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        named_mut(n as *mut lxb::LxbNode, lxb::lxb_dom_node_name)
    }

    /// The node carries a namespace id, so the URI is a lookup in the
    /// document's table - hence the document argument the XML binding
    /// ignores.
    #[inline]
    unsafe fn ns_uri<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        let mut len = 0usize;
        seen(lxb::mkr_html_ns_uri(n, doc, &mut len) as *const u8, len)
    }

    /// A strict unprefixed element test resolves in the HTML namespace, so
    /// only a genuinely foreign namespace (SVG, MathML) is a non-match -
    /// HTML and none both pass.
    #[inline]
    unsafe fn is_foreign_ns(_doc: Self::Doc, n: Self::Node) -> bool {
        (*n).ns != lxb::NS_HTML && (*n).ns != lxb::NS_UNDEF
    }
    #[inline]
    unsafe fn has_ns(_doc: Self::Doc, n: Self::Node) -> bool {
        (*n).ns != lxb::NS_UNDEF
    }

    /// Lexbor builds a node's text content on demand and hands back an
    /// allocation, so the append and the free stay together in C.
    #[inline]
    unsafe fn append_own_text(_doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int {
        lxb::mkr_html_append_own_text(n, buf)
    }

    /// The tag-id index, which is only an approximation of a name test:
    /// Lexbor's tag-name lookup has case and normalization quirks, so every
    /// candidate is re-checked. It is sound only for a pure-HTML document -
    /// in foreign content an element's qualified name need not equal its
    /// tag's canonical name, so a match could sit in another bucket - and it
    /// is keyed by tag id alone, so a prefixed test has no bucket.
    unsafe fn name_bucket<'a>(
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
        let doc = mkr_ctx_document(ctx) as *const lxb::Document;
        if doc.is_null() {
            return None;
        }
        let tag = lxb::mkr_html_tag_id_by_name(
            doc,
            local.as_ptr() as *const core::ffi::c_char,
            local.len(),
        );
        /* The index buckets only the static tag-id range; a custom element's
         * tag id is a pointer value, so those fall back to the walk and are
         * still found. */
        if tag == lxb::TAG_UNDEF || tag >= lxb::TAG_LAST_ENTRY {
            return None;
        }
        let mut cnt = 0usize;
        let bucket = lookup(index, tag, &mut cnt);
        let nodes = if bucket.is_null() || cnt == 0 {
            &[][..]
        } else {
            core::slice::from_raw_parts(bucket, cnt)
        };
        Some(Bucket {
            nodes,
            recheck: true,
        })
    }
}
