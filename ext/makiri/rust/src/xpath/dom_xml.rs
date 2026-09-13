//! The XML binding of the node-access contract.
//!
//! A node handle is an index-arena [`xml::NodeId`] and the storage is the
//! [`xml::Document`] that owns the slot array and byte store, so every access
//! resolves through the document. This is the one reviewed unsafe adapter of
//! the XML backend: it turns the document pointer the engine carries into
//! `&Document` and calls safe accessors; no raw node pointer is dereferenced.

#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::dom::{Bucket, Dom};
use crate::xml::abi as xml;
use core::ffi::c_int;

/// The XML storage handle: the index-arena document.
pub struct Xml;

#[inline]
unsafe fn d<'a>(doc: *mut xml::Document) -> &'a xml::Document {
    &*doc
}

/// A namespace declaration is a NAMESPACE node in XPath 1.0, not an attribute,
/// so it must not appear on the attribute axis. The reader still keeps it as a
/// DOM attribute (Node#attribute_nodes reads `attrs` directly, matching DOM
/// Level 2); only the XPath iteration below skips it.
unsafe fn is_ns_decl(doc: *mut xml::Document, a: xml::NodeId) -> bool {
    let q = d(doc).qname(a);
    q == b"xmlns" || q.starts_with(b"xmlns:")
}

unsafe fn skip_ns_decls(doc: *mut xml::Document, mut a: xml::NodeId) -> xml::NodeId {
    while !a.is_invalid() && is_ns_decl(doc, a) {
        a = d(doc).next(a).unwrap_or(xml::NodeId::INVALID);
    }
    a
}

unsafe impl Dom for Xml {
    type Node = xml::NodeId;
    type Doc = *mut xml::Document;

    const IS_XML: bool = true;

    #[inline]
    fn null() -> Self::Node {
        xml::NodeId::INVALID
    }
    #[inline]
    fn is_null(n: Self::Node) -> bool {
        n.is_invalid()
    }

    #[inline]
    fn to_void(n: Self::Node) -> *mut core::ffi::c_void {
        n.to_token() as *mut core::ffi::c_void
    }
    #[inline]
    unsafe fn from_void(p: *mut core::ffi::c_void) -> Self::Node {
        xml::NodeId::from_token(p as usize)
    }
    #[inline]
    unsafe fn doc_from_void(p: *mut core::ffi::c_void) -> Self::Doc {
        p as *mut xml::Document
    }

    #[inline]
    unsafe fn document_node(doc: Self::Doc) -> Self::Node {
        d(doc).doc_node()
    }

    #[inline]
    unsafe fn node_type(doc: Self::Doc, n: Self::Node) -> u32 {
        d(doc).type_(n)
    }

    #[inline]
    unsafe fn first_child(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc).first_child(n).unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn last_child(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc).last_child(n).unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn next(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc).next(n).unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn prev(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc).prev(n).unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn parent(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc).parent(n).unwrap_or(xml::NodeId::INVALID)
    }

    #[inline]
    unsafe fn first_attr(doc: Self::Doc, el: Self::Node) -> Self::Node {
        match d(doc).attrs(el) {
            Some(a) => skip_ns_decls(doc, a),
            None => xml::NodeId::INVALID,
        }
    }
    #[inline]
    unsafe fn attr_next(doc: Self::Doc, a: Self::Node) -> Self::Node {
        match d(doc).next(a) {
            Some(n) => skip_ns_decls(doc, n),
            None => xml::NodeId::INVALID,
        }
    }
    #[inline]
    unsafe fn attr_value<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        d(doc).value(a)
    }

    unsafe fn get_attribute<'a>(doc: Self::Doc, el: Self::Node, name: &[u8]) -> Option<&'a [u8]> {
        let mut a = d(doc).attrs(el);
        while let Some(attr) = a {
            if !is_ns_decl(doc, attr) && d(doc).qname(attr) == name {
                return Some(d(doc).value(attr));
            }
            a = d(doc).next(attr);
        }
        None
    }

    #[inline]
    unsafe fn local_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        d(doc).local(n)
    }
    #[inline]
    unsafe fn attr_local_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        d(doc).local(a)
    }
    #[inline]
    unsafe fn qualified_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        d(doc).qname(n)
    }
    #[inline]
    unsafe fn attr_qualified_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        d(doc).qname(a)
    }
    #[inline]
    unsafe fn pi_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        d(doc).local(n)
    }

    #[inline]
    unsafe fn ns_uri<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        d(doc).ns(n)
    }

    /// Any namespace URI is foreign to an unprefixed test: a strict unprefixed
    /// element test matches a no-namespace node only.
    #[inline]
    unsafe fn is_foreign_ns(doc: Self::Doc, n: Self::Node) -> bool {
        !d(doc).ns(n).is_empty()
    }

    #[inline]
    unsafe fn has_ns(doc: Self::Doc, n: Self::Node) -> bool {
        !d(doc).ns(n).is_empty()
    }

    /// The node owns its value, so this is an append of a borrowed slice.
    #[inline]
    unsafe fn append_own_text(doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int {
        let s = d(doc).value(n);
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
        let owner = mkr_ctx_document(ctx); /* the XML storage == the name index owner */
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
