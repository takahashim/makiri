//! The XML binding of the node-access contract.
//!
//! A node handle is an index-arena [`xml::NodeId`] and the storage is the
//! [`xml::Document`] that owns the slot array and byte store, so every access
//! resolves through the document. This is the one reviewed unsafe adapter of
//! the XML backend: it turns the document pointer the engine carries into
//! `&Document` and calls safe accessors.
//!
//! Every handle here may have come from a node-set token, which is not
//! authenticated, so it is resolved through [`xml::Document::try_node`]: a
//! handle naming another document, a dropped slot or an out-of-range index
//! yields no node, and the operation reports the null node / empty bytes /
//! false rather than panicking.

#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::dom::{Bucket, Dom};
use crate::xml::model as xml;
use core::ffi::c_int;

/// The XML storage handle: the index-arena document.
pub struct Xml;

#[inline]
unsafe fn d<'a>(doc: *mut xml::Document) -> Option<&'a xml::Document> {
    doc.as_ref()
}

/// Resolve `id`, fail-closed. `None` for a null document, the null handle, an
/// out-of-range index, or a handle whose document stamp is not this one.
#[inline]
unsafe fn nd<'a>(doc: *mut xml::Document, id: xml::NodeId) -> Option<&'a xml::Node> {
    d(doc)?.try_node(id)
}

#[inline]
unsafe fn span<'a>(doc: *mut xml::Document, s: xml::Span) -> &'a [u8] {
    match d(doc) {
        Some(dd) => dd.span(s),
        None => &[],
    }
}

/// A namespace declaration is a NAMESPACE node in XPath 1.0, not an attribute,
/// so it must not appear on the attribute axis. The reader still keeps it as a
/// DOM attribute (Node#attribute_nodes reads `attrs` directly, matching DOM
/// Level 2); only the XPath iteration below skips it.
unsafe fn is_ns_decl(doc: *mut xml::Document, a: xml::NodeId) -> bool {
    let q = span(doc, nd(doc, a).map_or(xml::Span::ABSENT, |x| x.qname));
    q == b"xmlns" || q.starts_with(b"xmlns:")
}

unsafe fn skip_ns_decls(doc: *mut xml::Document, mut a: xml::NodeId) -> xml::NodeId {
    let Some(dd) = d(doc) else {
        return xml::NodeId::INVALID;
    };
    while !a.is_invalid() && is_ns_decl(doc, a) {
        a = dd.next(a).unwrap_or(xml::NodeId::INVALID);
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
        d(doc).map_or(xml::NodeId::INVALID, |dd| dd.doc_node())
    }

    #[inline]
    unsafe fn node_type(doc: Self::Doc, n: Self::Node) -> u32 {
        nd(doc, n).map_or(0, |x| x.type_.as_u32())
    }

    #[inline]
    unsafe fn first_child(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.first_child(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn last_child(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.last_child(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn next(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.next(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn prev(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.prev(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn parent(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.parent(n))
            .unwrap_or(xml::NodeId::INVALID)
    }

    #[inline]
    unsafe fn first_attr(doc: Self::Doc, el: Self::Node) -> Self::Node {
        match d(doc).and_then(|dd| dd.attrs(el)) {
            Some(a) => skip_ns_decls(doc, a),
            None => xml::NodeId::INVALID,
        }
    }
    #[inline]
    unsafe fn attr_next(doc: Self::Doc, a: Self::Node) -> Self::Node {
        match d(doc).and_then(|dd| dd.next(a)) {
            Some(n) => skip_ns_decls(doc, n),
            None => xml::NodeId::INVALID,
        }
    }
    #[inline]
    unsafe fn attr_value<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        match nd(doc, a) {
            Some(x) => span(doc, x.value),
            None => &[],
        }
    }

    unsafe fn get_attribute<'a>(doc: Self::Doc, el: Self::Node, name: &[u8]) -> Option<&'a [u8]> {
        let mut a = d(doc).and_then(|dd| dd.attrs(el));
        while let Some(id) = a {
            if !is_ns_decl(doc, id)
                && span(doc, nd(doc, id).map_or(xml::Span::ABSENT, |x| x.qname)) == name
            {
                return Some(span(
                    doc,
                    nd(doc, id).map_or(xml::Span::ABSENT, |x| x.value),
                ));
            }
            a = d(doc).and_then(|dd| dd.next(id));
        }
        None
    }

    #[inline]
    unsafe fn local_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.local),
            None => &[],
        }
    }
    #[inline]
    unsafe fn attr_local_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        match nd(doc, a) {
            Some(x) => span(doc, x.local),
            None => &[],
        }
    }
    #[inline]
    unsafe fn qualified_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.qname),
            None => &[],
        }
    }
    #[inline]
    unsafe fn attr_qualified_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        match nd(doc, a) {
            Some(x) => span(doc, x.qname),
            None => &[],
        }
    }
    #[inline]
    unsafe fn pi_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.local),
            None => &[],
        }
    }

    #[inline]
    unsafe fn ns_uri<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.ns_uri),
            None => &[],
        }
    }

    /// Any namespace URI is foreign to an unprefixed test: a strict unprefixed
    /// element test matches a no-namespace node only.
    #[inline]
    unsafe fn is_foreign_ns(doc: Self::Doc, n: Self::Node) -> bool {
        nd(doc, n).is_some_and(|x| x.ns_uri.len != 0)
    }

    #[inline]
    unsafe fn has_ns(doc: Self::Doc, n: Self::Node) -> bool {
        nd(doc, n).is_some_and(|x| x.ns_uri.len != 0)
    }

    /// The node owns its value, so this is an append of a borrowed slice.
    #[inline]
    unsafe fn append_own_text(doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int {
        let s = match nd(doc, n) {
            Some(x) => span(doc, x.value),
            None => return MKR_OK,
        };
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
