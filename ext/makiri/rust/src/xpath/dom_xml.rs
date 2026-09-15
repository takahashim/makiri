//! The XML binding of the node-access contract.
//!
//! A node handle is an opaque, stamped [`xml::NodeId`]. It is not a slot index:
//! the stamp identifies the owning [`xml::Document`], and the low bits identify
//! the arena slot. The adapter never indexes the arena directly; every access
//! resolves through `Document`'s checked API.
//!
//! The only unsafe boundary here is the erased document pointer and the
//! NodeId-to-void token conversion required by the shared XPath ABI. Once that
//! boundary is crossed, navigation and byte access use the safe XML API.
//!
//! A handle coming from a node-set token is untrusted. `try_node` therefore
//! rejects a token naming another document, an invalid or out-of-range slot,
//! and any stale handle. The operation then reports the null node, empty bytes,
//! or `false`, never a panic or a cross-document access.

#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use super::dom::{Bucket, DomHandle, DomRaw};
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

unsafe impl DomHandle for Xml {
    type Node = xml::NodeId;
    type Doc = *mut xml::Document;

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
    /// The token is opaque ABI data; validation happens when `NodeId` is
    /// resolved against the document by `nd`.
    #[inline]
    unsafe fn from_void(p: *mut core::ffi::c_void) -> Self::Node {
        xml::NodeId::from_token(p as usize)
    }
    /// The document pointer is validated for nullness by `d`; node ownership
    /// is validated later by `Document::try_node`.
    unsafe fn doc_from_void(p: *mut core::ffi::c_void) -> Self::Doc {
        p as *mut xml::Document
    }
}

unsafe impl DomRaw for Xml {
    const RAW_IS_XML: bool = true;

    #[inline]
    unsafe fn raw_document_node(doc: Self::Doc) -> Self::Node {
        d(doc).map_or(xml::NodeId::INVALID, |dd| dd.doc_node())
    }

    #[inline]
    unsafe fn raw_node_type(doc: Self::Doc, n: Self::Node) -> u32 {
        nd(doc, n).map_or(0, |x| x.type_.as_u32())
    }

    #[inline]
    unsafe fn raw_first_child(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.first_child(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn raw_last_child(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.last_child(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn raw_next(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.next(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn raw_prev(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.prev(n))
            .unwrap_or(xml::NodeId::INVALID)
    }
    #[inline]
    unsafe fn raw_parent(doc: Self::Doc, n: Self::Node) -> Self::Node {
        d(doc)
            .and_then(|dd| dd.parent(n))
            .unwrap_or(xml::NodeId::INVALID)
    }

    #[inline]
    unsafe fn raw_first_attr(doc: Self::Doc, el: Self::Node) -> Self::Node {
        match d(doc).and_then(|dd| dd.attrs(el)) {
            Some(a) => skip_ns_decls(doc, a),
            None => xml::NodeId::INVALID,
        }
    }
    #[inline]
    unsafe fn raw_attr_next(doc: Self::Doc, a: Self::Node) -> Self::Node {
        match d(doc).and_then(|dd| dd.next(a)) {
            Some(n) => skip_ns_decls(doc, n),
            None => xml::NodeId::INVALID,
        }
    }
    #[inline]
    unsafe fn raw_attr_value<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        match nd(doc, a) {
            Some(x) => span(doc, x.value),
            None => &[],
        }
    }

    unsafe fn raw_get_attribute<'a>(
        doc: Self::Doc,
        el: Self::Node,
        name: &[u8],
    ) -> Option<&'a [u8]> {
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
    unsafe fn raw_local_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.local),
            None => &[],
        }
    }
    #[inline]
    unsafe fn raw_attr_local_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        match nd(doc, a) {
            Some(x) => span(doc, x.local),
            None => &[],
        }
    }
    #[inline]
    unsafe fn raw_qualified_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.qname),
            None => &[],
        }
    }
    #[inline]
    unsafe fn raw_attr_qualified_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8] {
        match nd(doc, a) {
            Some(x) => span(doc, x.qname),
            None => &[],
        }
    }
    #[inline]
    unsafe fn raw_pi_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.local),
            None => &[],
        }
    }

    #[inline]
    unsafe fn raw_ns_uri<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8] {
        match nd(doc, n) {
            Some(x) => span(doc, x.ns_uri),
            None => &[],
        }
    }

    /// Any namespace URI is foreign to an unprefixed test: a strict unprefixed
    /// element test matches a no-namespace node only.
    #[inline]
    unsafe fn raw_is_foreign_ns(doc: Self::Doc, n: Self::Node) -> bool {
        nd(doc, n).is_some_and(|x| x.ns_uri.len != 0)
    }

    #[inline]
    unsafe fn raw_has_ns(doc: Self::Doc, n: Self::Node) -> bool {
        nd(doc, n).is_some_and(|x| x.ns_uri.len != 0)
    }

    /// The node owns its value, so this is an append of a borrowed slice.
    #[inline]
    unsafe fn raw_append_own_text(doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int {
        let s = match nd(doc, n) {
            Some(x) => span(doc, x.value),
            None => return BUF_OK,
        };
        if s.is_empty() {
            return BUF_OK;
        }
        buf_append(buf, s.as_ptr() as *const core::ffi::c_void, s.len())
    }

    /// The XML name index is keyed by (local name, namespace URI), so a bucket
    /// holds exactly the matching elements and needs no re-check. An unprefixed
    /// LAX test means "any namespace", which one bucket cannot express, so it
    /// falls back to the walk.
    unsafe fn raw_name_bucket<'a>(
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
        if !matches!(ctx_backend(ctx), Some(Backend::Xml)) {
            return None;
        }
        /* The XML storage owns the index. */
        let doc = (ctx_document(ctx) as *mut xml::Document).as_mut()?;
        /* Built lazily and cached; None on OOM, and the caller walks. */
        let idx = crate::xml::index::get(doc)?;
        let ids = crate::xml::index::lookup(idx, local, uri);
        // SAFETY: `NodeId` is one word and is exactly the opaque token the engine
        // carries. The borrow stays valid until the next mutation invalidates the
        // index; the engine consumes it only during this GVL-held evaluate.
        let nodes =
            core::slice::from_raw_parts(ids.as_ptr() as *const *mut core::ffi::c_void, ids.len());
        Some(Bucket {
            nodes,
            recheck: false,
        })
    }
}
