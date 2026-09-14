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
use core::ffi::c_int;

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

/// The raw-handle boundary shared by each DOM representation.
///
/// Only this part converts the erased pointers used by the context and
/// node-set ABI back into backend handles. The safety contract is deliberately
/// separate from [`Dom`], whose methods describe DOM operations.
pub unsafe trait DomHandle {
    type Node: Copy + PartialEq;
    type Doc: Copy;

    fn null() -> Self::Node;
    fn is_null(n: Self::Node) -> bool;
    fn to_void(n: Self::Node) -> *mut core::ffi::c_void;

    /// `p` must be a handle produced by this backend, or null.
    unsafe fn from_void(p: *mut core::ffi::c_void) -> Self::Node;
    /// `p` must be storage produced by this backend, or null.
    unsafe fn doc_from_void(p: *mut core::ffi::c_void) -> Self::Doc;
}

/// One DOM representation, as the engine needs to see it.
///
/// The two backends differ in what a handle IS, and each states it:
///
/// - **HTML** (`Html`): a `*mut lxb_dom_node_t` into a Lexbor tree the engine
///   does not own, self-contained (Lexbor nodes carry their own links). Every
///   method is unsafe because it dereferences that pointer; the caller promises
///   the handle is live.
/// - **XML** (`Xml`): an index-arena `NodeId`, not a pointer. The links and
///   bytes live in the `Document` the method also receives, and each is
///   resolved through `Document::try_node`, which fails closed (null node /
///   empty bytes) for an out-of-range, stale or foreign-document handle. So the
///   XML methods are unsafe only by the trait's signature, not because they
///   dereference anything.
///
/// # Safety
/// An implementation must report navigation that forms an actual tree - a
/// child's parent is the node it was reached from, siblings agree on order -
/// because the engine derives document order from it.
pub unsafe trait Dom: DomHandle {
    /// Selects the host-policy branches the C spells `#ifdef MKR_HOST_XML`:
    /// `id()` is the empty node-set in XML (an ID is DTD-declared, and DTDs are
    /// rejected at parse), `lang()` reads xml:lang rather than HTML's `lang`,
    /// and the CSS-lowered of-type hooks exist only for XML.
    const IS_XML: bool;

    /// The document node itself, for a walk rooted at the whole tree.
    unsafe fn document_node(doc: Self::Doc) -> Self::Node;

    unsafe fn node_type(doc: Self::Doc, n: Self::Node) -> u32;

    /* navigation */
    unsafe fn first_child(doc: Self::Doc, n: Self::Node) -> Self::Node;
    unsafe fn last_child(doc: Self::Doc, n: Self::Node) -> Self::Node;
    unsafe fn next(doc: Self::Doc, n: Self::Node) -> Self::Node;
    unsafe fn prev(doc: Self::Doc, n: Self::Node) -> Self::Node;
    unsafe fn parent(doc: Self::Doc, n: Self::Node) -> Self::Node;

    /* attributes - iteration yields attribute handles, which are node handles
     * in both representations (the C contract's MKR_ELEM_FIRST_ATTR /
     * MKR_ATTR_NEXT). */
    unsafe fn first_attr(doc: Self::Doc, el: Self::Node) -> Self::Node;
    unsafe fn attr_next(doc: Self::Doc, a: Self::Node) -> Self::Node;
    unsafe fn attr_value<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8];
    /// Attribute value by raw qualified name, or None.
    unsafe fn get_attribute<'a>(doc: Self::Doc, el: Self::Node, name: &[u8]) -> Option<&'a [u8]>;

    /* names (borrowed from the tree) */
    unsafe fn local_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8];
    unsafe fn attr_local_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8];
    unsafe fn qualified_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8];
    unsafe fn attr_qualified_name<'a>(doc: Self::Doc, a: Self::Node) -> &'a [u8];
    unsafe fn pi_name<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8];

    /// The node's namespace URI, empty if it has none.
    unsafe fn ns_uri<'a>(doc: Self::Doc, n: Self::Node) -> &'a [u8];

    /// True when a strict unprefixed element name test must NOT match this
    /// node: XML calls any namespace foreign, HTML admits its own and none.
    unsafe fn is_foreign_ns(doc: Self::Doc, n: Self::Node) -> bool;

    /// Whether the node is in a namespace at all - `MKR_NODE_NS_ID(n) != 0`.
    /// Separate from `ns_uri` because HTML answers it without the document.
    unsafe fn has_ns(doc: Self::Doc, n: Self::Node) -> bool;

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
    unsafe fn append_own_text(doc: Self::Doc, n: Self::Node, buf: *mut Buf) -> c_int;

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
