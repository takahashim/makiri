//! The XML binding of the node-access contract (mkr_xpath_node_access_xml.h).
//!
//! The node is its own element handle, it carries its namespace URI and a
//! contiguous "prefix:local" qname directly, and attributes hang off `attrs` as
//! a sibling list.

/* The trait states the precondition once, for every method: a handle is a raw
 * pointer into a tree the engine does not own, so the caller promises it is
 * live and belongs to the document being evaluated. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::dom::{Bucket, Dom};
use crate::xml::abi as xml;
use core::ffi::c_int;
use core::ptr;

/// `mkr_xml_node_t`, from mkr_xpath_node_access_xml.h. The node is its own
/// element handle, it carries its namespace URI and a contiguous "prefix:local"
/// qname directly, and attributes hang off `attrs` as a sibling list.
pub struct Xml;

/// A namespace declaration is a NAMESPACE node in XPath 1.0, not an attribute,
/// so it must not appear on the attribute axis. The reader still keeps it as a
/// DOM attribute (Node#attribute_nodes reads `attrs` directly, matching DOM
/// Level 2); only the XPath iteration below skips it.
unsafe fn is_ns_decl(a: *const xml::Node) -> bool {
    let q = xml::node_qname(a);
    q == b"xmlns" || q.starts_with(b"xmlns:")
}

unsafe fn skip_ns_decls(mut a: *mut xml::Node) -> *mut xml::Node {
    while !a.is_null() && is_ns_decl(a) {
        a = (*a).next;
    }
    a
}

unsafe impl Dom for Xml {
    type Node = *mut xml::Node;
    type Doc = *mut xml::Doc;

    const IS_XML: bool = true;

    #[inline]
    fn null() -> Self::Node {
        ptr::null_mut()
    }
    #[inline]
    fn is_null(n: Self::Node) -> bool {
        n.is_null()
    }

    #[inline]
    fn to_void(n: Self::Node) -> *mut core::ffi::c_void {
        n as *mut core::ffi::c_void
    }
    #[inline]
    unsafe fn from_void(p: *mut core::ffi::c_void) -> Self::Node {
        p as Self::Node
    }
    #[inline]
    unsafe fn doc_from_void(p: *mut core::ffi::c_void) -> Self::Doc {
        p as Self::Doc
    }

    #[inline]
    unsafe fn node_type(n: Self::Node) -> u32 {
        (*n).type_
    }

    #[inline]
    unsafe fn first_child(n: Self::Node) -> Self::Node {
        (*n).first_child
    }
    #[inline]
    unsafe fn last_child(n: Self::Node) -> Self::Node {
        (*n).last_child
    }
    #[inline]
    unsafe fn next(n: Self::Node) -> Self::Node {
        (*n).next
    }
    #[inline]
    unsafe fn prev(n: Self::Node) -> Self::Node {
        (*n).prev
    }
    #[inline]
    unsafe fn parent(n: Self::Node) -> Self::Node {
        (*n).parent
    }

    #[inline]
    unsafe fn first_attr(el: Self::Node) -> Self::Node {
        skip_ns_decls((*el).attrs)
    }
    #[inline]
    unsafe fn attr_next(a: Self::Node) -> Self::Node {
        skip_ns_decls((*a).next)
    }
    #[inline]
    unsafe fn attr_value<'a>(a: Self::Node) -> &'a [u8] {
        xml::node_value(a)
    }

    unsafe fn get_attribute<'a>(el: Self::Node, name: &[u8]) -> Option<&'a [u8]> {
        let mut a = (*el).attrs;
        while !a.is_null() {
            if !is_ns_decl(a) && xml::node_qname(a) == name {
                return Some(xml::node_value(a));
            }
            a = (*a).next;
        }
        None
    }

    #[inline]
    unsafe fn local_name<'a>(n: Self::Node) -> &'a [u8] {
        xml::node_local(n)
    }
    #[inline]
    unsafe fn attr_local_name<'a>(a: Self::Node) -> &'a [u8] {
        xml::node_local(a)
    }
    #[inline]
    unsafe fn qualified_name<'a>(n: Self::Node) -> &'a [u8] {
        xml::node_qname(n)
    }
    #[inline]
    unsafe fn attr_qualified_name<'a>(a: Self::Node) -> &'a [u8] {
        xml::node_qname(a)
    }
    #[inline]
    unsafe fn pi_name<'a>(n: Self::Node) -> &'a [u8] {
        xml::node_local(n)
    }

    /// The node holds the resolved URI, so the document is unused.
    #[inline]
    unsafe fn ns_uri<'a>(n: Self::Node, _doc: Self::Doc) -> &'a [u8] {
        xml::node_ns(n)
    }

    /// Any namespace URI is foreign to an unprefixed test: a strict unprefixed
    /// element test matches a no-namespace node only.
    #[inline]
    unsafe fn is_foreign_ns(n: Self::Node) -> bool {
        (*n).ns_uri_len != 0
    }

    #[inline]
    unsafe fn has_ns(n: Self::Node) -> bool {
        (*n).ns_uri_len != 0
    }

    /// The node owns its value, so this is an append of a borrowed slice.
    #[inline]
    unsafe fn append_own_text(n: Self::Node, buf: *mut Buf) -> c_int {
        let s = xml::node_value(n);
        if s.is_empty() {
            return MKR_OK;
        }
        mkr_buf_append(buf, s.as_ptr() as *const core::ffi::c_void, s.len())
    }

    /// The XML name index is keyed by (local name, namespace URI), so a bucket
    /// holds exactly the matching elements and needs no re-check. An unprefixed
    /// LAX test means "any namespace", which one bucket cannot express, so it
    /// falls back to the walk.
    unsafe fn name_bucket<'a>(
        ctx: *mut Context,
        local: &[u8],
        ns_uri: Option<&[u8]>,
        lax: bool,
    ) -> Option<Bucket<'a>> {
        let uri = match ns_uri {
            Some(u) => u,
            None if lax => return None,
            None => b"", /* strict unprefixed -> no namespace */
        };
        let owner = mkr_ctx_name_index_owner(ctx);
        let get = mkr_ctx_name_index_get(ctx)?;
        let lookup = mkr_ctx_name_index_lookup(ctx)?;
        if owner.is_null() {
            return None;
        }
        let idx = get(owner); /* lazily builds and caches; NULL on OOM */
        if idx.is_null() {
            return None;
        }
        let mut cnt = 0usize;
        let bucket = lookup(
            idx,
            local.as_ptr() as *const core::ffi::c_char,
            local.len(),
            uri.as_ptr() as *const core::ffi::c_char,
            uri.len(),
            &mut cnt,
        );
        let nodes = if bucket.is_null() || cnt == 0 {
            &[][..]
        } else {
            core::slice::from_raw_parts(bucket, cnt)
        };
        Some(Bucket {
            nodes,
            recheck: false,
        })
    }
}
