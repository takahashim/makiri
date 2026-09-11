//! The node-access contract the engine is written against, and the XML binding
//! of it.
//!
//! In C this is a pair of headers of `MKR_NODE_*` macros
//! (mkr_xpath_node_access_{html,xml}.h) plus a prelude that `#include`s the
//! engine bodies once per representation. That is monomorphization by
//! preprocessor: it costs nothing at runtime (a runtime kind-branch measured
//! ~+150%), but nothing checks that the two bindings agree on what an operation
//! means, or that the body only uses operations both provide.
//!
//! A trait is the same monomorphization with those two things checked. The
//! engine is generic over `Dom`, so each backend is compiled into its own copy
//! with the calls inlined - and a backend that forgets an operation, or gives it
//! the wrong type, does not build.

/* The trait states the precondition once, for every method: a handle is a raw
 * pointer into a tree the engine does not own, so the caller promises it is
 * live and belongs to the document being evaluated. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use crate::xml::abi as xml;
use core::ffi::c_int;
use core::ptr;

/* ---- node types (shared numeric encoding) ----
 *
 * The whole monomorphization rests on the two representations agreeing on the
 * node-type encoding, so a node's `type` integer means the same thing whichever
 * backend walks it. The C header asserts that equality against Lexbor's
 * LXB_DOM_NODE_TYPE_*; the values below are the same ones. */
pub const NTYPE_ELEMENT: u32 = 1;
pub const NTYPE_ATTRIBUTE: u32 = 2;
pub const NTYPE_TEXT: u32 = 3;
pub const NTYPE_CDATA_SECTION: u32 = 4;
pub const NTYPE_ENTITY_REFERENCE: u32 = 5;
pub const NTYPE_ENTITY: u32 = 6;
pub const NTYPE_PI: u32 = 7;
pub const NTYPE_COMMENT: u32 = 8;
pub const NTYPE_DOCUMENT: u32 = 9;
pub const NTYPE_DOCUMENT_TYPE: u32 = 10;
pub const NTYPE_NOTATION: u32 = 12;

/// One DOM representation, as the engine needs to see it.
///
/// Handles are raw pointers into a tree the engine does not own, so every
/// method is unsafe: the caller promises the handle is live and belongs to the
/// document being evaluated. That is the same promise the C macros made
/// silently.
///
/// # Safety
/// An implementation must report navigation that forms an actual tree - a
/// child's parent is the node it was reached from, siblings agree on order -
/// because the engine derives document order from it.
pub unsafe trait Dom {
    /// A node handle. `Copy` so the engine can move it around freely; equality
    /// is pointer identity, which is what node-set dedup keys on.
    type Node: Copy + PartialEq;

    /// Selects the host-policy branches the C spells `#ifdef MKR_HOST_XML`:
    /// `id()` is the empty node-set in XML (an ID is DTD-declared, and DTDs are
    /// rejected at parse), `lang()` reads xml:lang rather than HTML's `lang`,
    /// and the CSS-lowered of-type hooks exist only for XML.
    const IS_XML: bool;

    /// The owning document, for the services that need it (namespace lookup on
    /// HTML resolves an id against the document's table).
    type Doc: Copy;

    fn null() -> Self::Node;
    fn is_null(n: Self::Node) -> bool;

    /* A node-set stores `void *` because it crosses into the glue and the
     * custom-function bridge, which do not know the representation. These two
     * are where that erasure happens, so each backend - and only each backend -
     * states how its handle maps to a pointer. */
    fn to_void(n: Self::Node) -> *mut core::ffi::c_void;
    /// # Safety
    /// `p` must be a handle this backend produced, or null.
    unsafe fn from_void(p: *mut core::ffi::c_void) -> Self::Node;
    /// The same erasure for the document handle `mkr_ctx_document` returns.
    ///
    /// # Safety
    /// `p` must be this backend's document, or null.
    unsafe fn doc_from_void(p: *mut core::ffi::c_void) -> Self::Doc;

    unsafe fn node_type(n: Self::Node) -> u32;

    /* navigation */
    unsafe fn first_child(n: Self::Node) -> Self::Node;
    unsafe fn last_child(n: Self::Node) -> Self::Node;
    unsafe fn next(n: Self::Node) -> Self::Node;
    unsafe fn prev(n: Self::Node) -> Self::Node;
    unsafe fn parent(n: Self::Node) -> Self::Node;

    /* attributes - iteration yields attribute handles, which are node handles
     * in both representations (the C contract's MKR_ELEM_FIRST_ATTR /
     * MKR_ATTR_NEXT). */
    unsafe fn first_attr(el: Self::Node) -> Self::Node;
    unsafe fn attr_next(a: Self::Node) -> Self::Node;
    unsafe fn attr_value<'a>(a: Self::Node) -> &'a [u8];
    /// Attribute value by raw qualified name, or None.
    unsafe fn get_attribute<'a>(el: Self::Node, name: &[u8]) -> Option<&'a [u8]>;

    /* names (borrowed from the tree) */
    unsafe fn local_name<'a>(n: Self::Node) -> &'a [u8];
    unsafe fn attr_local_name<'a>(a: Self::Node) -> &'a [u8];
    unsafe fn qualified_name<'a>(n: Self::Node) -> &'a [u8];
    unsafe fn attr_qualified_name<'a>(a: Self::Node) -> &'a [u8];
    unsafe fn pi_name<'a>(n: Self::Node) -> &'a [u8];

    /// The node's namespace URI, empty if it has none.
    unsafe fn ns_uri<'a>(n: Self::Node, doc: Self::Doc) -> &'a [u8];

    /// True when a strict unprefixed element name test must NOT match this
    /// node: XML calls any namespace foreign, HTML admits its own and none.
    unsafe fn is_foreign_ns(n: Self::Node) -> bool;

    /// Whether the node is in a namespace at all - `MKR_NODE_NS_ID(n) != 0`.
    /// Separate from `ns_uri` because HTML answers it without the document.
    unsafe fn has_ns(n: Self::Node) -> bool;

    /// Append the node's own text - the bytes it contributes to a string-value -
    /// to `buf`, returning an `mkr_status_t`.
    ///
    /// It appends rather than returning a slice because only one backend can
    /// lend those bytes. The XML node owns its value, but Lexbor builds a node's
    /// text content on demand and hands back an allocation the caller must free,
    /// so a borrowed return has nowhere to free it. Owning the append is the one
    /// shape both can satisfy - and it is what the C contract says
    /// (`MKR_NODE_APPEND_OWN_TEXT`, which is a statement, not an expression, for
    /// exactly this reason).
    unsafe fn append_own_text(n: Self::Node, buf: *mut Buf) -> c_int;

    /// The document-level element index's answer for a document-rooted,
    /// predicate-free descendant name test, or None when it cannot serve one.
    ///
    /// Both hosts keep such an index, but they key it differently: XML by
    /// (local name, namespace URI), which is exactly the test, and HTML by
    /// Lexbor tag id, which is only an approximation - hence `recheck`.
    ///
    /// # Safety
    /// `ctx` must be the evaluating context.
    unsafe fn name_bucket<'a>(
        ctx: *mut Context,
        local: &[u8],
        ns_uri: Option<&[u8]>,
        lax: bool,
    ) -> Option<Bucket<'a>>;
}

/// What `Dom::name_bucket` found: the elements, in document order, and whether
/// each still has to be re-checked against the name test before it counts. The
/// index hands back `void *` (it is shared with the glue), so the handles are
/// erased here too and converted one at a time.
pub struct Bucket<'a> {
    pub nodes: &'a [*mut core::ffi::c_void],
    pub recheck: bool,
}

/* ---- the XML binding ---- */

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
        Some(Bucket { nodes, recheck: false })
    }
}
