//! The Ruby <-> Lexbor DOM seam.
//!
//! This is the one module that depends on both the Ruby ABI ([`crate::bridge::ruby`],
//! [`crate::bridge::typed`]) and the Lexbor ABI ([`crate::lexbor`]). That is why
//! it lives under `bridge`: a value read out of a Ruby wrapper becomes a live
//! Lexbor handle here, and the conversion is unsafe. The representation wrapper
//! structs (`mkr_node_data_t`, `mkr_doc_data_t`), their TypedData and GC hooks,
//! and the accessors glue calls all live here, so no glue module writes the raw
//! side of that conversion.
//!
//! The lexbor side stays Ruby-free (it knows no `Value`); this layer is the
//! bridge's Lexbor half, not the other way round.

#![allow(unsafe_code)]

use core::ffi::{c_int, c_void};

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, Value};

use crate::bridge::ruby::{nil, typed_data_known_ref, typed_data_ref, value, DataType, VALUE};
use crate::bridge::typed::{data_type, kind_of, Hooks, Marker};
use crate::init::{
    CLASS_DOCUMENT, CLASS_HTML_ATTR, CLASS_HTML_CDATA_SECTION, CLASS_HTML_COMMENT,
    CLASS_HTML_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT, CLASS_HTML_NODE,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_TEXT, CLASS_XML_ATTR, CLASS_XML_CDATA_SECTION,
    CLASS_XML_COMMENT, CLASS_XML_DOCUMENT, CLASS_XML_DOCUMENT_FRAGMENT, CLASS_XML_DOCUMENT_TYPE,
    CLASS_XML_ELEMENT, CLASS_XML_NODE, CLASS_XML_PROCESSING_INSTRUCTION, CLASS_XML_TEXT, EXC_ERROR,
};
use crate::lexbor::adapter::html::{
    HtmlNode, RawDoc, RawNode, TYPE_ATTRIBUTE, TYPE_CDATA, TYPE_COMMENT, TYPE_DOCTYPE,
    TYPE_DOCUMENT, TYPE_ELEMENT, TYPE_FRAGMENT, TYPE_PI, TYPE_TEXT,
};
use crate::lexbor::adapter::post_parse::Parsed;
use crate::xml::model::{Doc as XmlDoc, NodeId, NodeType};

/* ------------------------------------------------------------------ *
 * the node wrapper                                                   *
 * ------------------------------------------------------------------ */

/// `mkr_node_data_t`: the node pointer plus the keepalive Document.
///
/// The node is owned by the document's arena (HTML or XML), so the wrapper
/// never frees it; the Document reference is what keeps it alive, and marking
/// it is the wrapper's whole GC job.
pub struct NodeData {
    /// Representation-opaque; read it only through a kind-checked accessor.
    pub node: *mut c_void,
    pub document: VALUE,
}

impl Hooks for NodeData {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.document);
    }
}

pub static NODE_DATA_TYPE: DataType =
    data_type::<NodeData>(c"Makiri::Node".as_ptr(), core::ptr::null());

pub static HTML_NODE_TYPE: DataType =
    data_type::<NodeData>(c"Makiri::HTML::Node".as_ptr(), NODE_DATA_TYPE.as_ptr());

pub static XML_NODE_TYPE: DataType =
    data_type::<NodeData>(c"Makiri::XML::Node".as_ptr(), NODE_DATA_TYPE.as_ptr());

/// `NodeKind`.
const NODE_KIND_OTHER: c_int = 0;
const NODE_KIND_HTML: c_int = 1;
const NODE_KIND_XML: c_int = 2;

/* ------------------------------------------------------------------ *
 * the document wrapper                                               *
 * ------------------------------------------------------------------ */

/// `mkr_doc_data_t`: the parsed handle (owned - GC frees it) and the reserved
/// errors Array.
pub struct DocData {
    pub parsed: *mut Parsed,
    pub errors: VALUE,
}

impl Hooks for DocData {
    fn mark(&self, marker: &Marker) {
        marker.mark(self.errors);
    }

    fn memsize(&self) -> usize {
        let mut total = core::mem::size_of::<DocData>();
        // SAFETY: `parsed` is owned by this object and live for the call.
        unsafe {
            if let Some(xdoc) = self.parsed.as_ref().and_then(|p| p.xml_doc_ref()) {
                total += crate::xml::api::xml_doc_memsize(xdoc);
            }
        }
        total
    }

    fn release(&mut self) {
        if !self.parsed.is_null() {
            // SAFETY: `parsed` came from `Box::into_raw` and only this owns it.
            unsafe { drop(Box::from_raw(self.parsed)) };
        }
    }
}

/// The base type: the kind-agnostic accessors (`doc_parsed`, `#errors`) accept
/// either representation.
pub static DOC_TYPE: DataType =
    data_type::<DocData>(c"Makiri::Document".as_ptr(), core::ptr::null());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as a Lexbor document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
pub static HTML_DOC_TYPE: DataType =
    data_type::<DocData>(c"Makiri::HTML::Document".as_ptr(), DOC_TYPE.as_ptr());
pub static XML_DOC_TYPE: DataType =
    data_type::<DocData>(c"Makiri::XML::Document".as_ptr(), DOC_TYPE.as_ptr());

/// Wrap an owned parsed handle as a Document; GC takes ownership. The leaf
/// class follows the handle's kind.
///
/// # Safety
/// `parsed` must be an owned handle that nothing else frees.
pub unsafe fn wrap_document(parsed: *mut Parsed) -> VALUE {
    let html = !unsafe { (*parsed).is_xml() };
    let obj = new_document(html);
    set_document_parsed(obj, parsed);
    obj
}

/// A Document wrapper whose parsed handle is not set yet - for a parse that
/// fills it afterwards, so a failed parse still frees cleanly through the GC.
pub fn new_document(html: bool) -> VALUE {
    let (klass, ty) = if html {
        (crate::init::CLASS_HTML_DOCUMENT.raw(), &HTML_DOC_TYPE)
    } else {
        (crate::init::CLASS_XML_DOCUMENT.raw(), &XML_DOC_TYPE)
    };
    /* The errors array is built before the wrap and kept in a local, so the
     * conservative stack scan pins it across the wrap's allocation and the
     * store closure does not allocate (an allocation there could raise
     * NoMemoryError while the caller holds a live resource). */
    let errors = crate::bridge::ruby::array_new();
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    unsafe {
        crate::bridge::ruby::wrap_zeroed::<DocData>(
            klass,
            ty.as_ptr(),
            |d| d.parsed = core::ptr::null_mut(),
            |d| d.errors = errors.as_raw(),
        )
    }
}

/// Store the parsed handle into a Document wrapper built by [`new_document`].
pub fn set_document_parsed(rb_doc: VALUE, parsed: *mut Parsed) {
    // SAFETY: `rb_doc` is a Document (the base type matches either leaf).
    unsafe {
        let d = crate::bridge::ruby::typed_data_known(value(rb_doc), &DOC_TYPE) as *mut DocData;
        (*d).parsed = parsed;
    }
}

/* ------------------------------------------------------------------ *
 * lexbor <-> wrapper accessors                                       *
 * ------------------------------------------------------------------ */

/// The XML arena behind a parsed handle, or null for an HTML one.
///
/// # Safety
/// `p` must be a live handle.
pub unsafe fn parsed_xml_doc(p: *mut Parsed) -> *mut XmlDoc {
    // SAFETY: the caller's contract.
    unsafe { (*p).xml_doc() }
}

/// The Lexbor document behind an HTML Document. `Err(TypeError)` otherwise.
pub fn html_doc_unwrap(rb_doc: Value) -> Result<RawDoc, Error> {
    let d: &DocData = typed_data_ref(rb_doc, &HTML_DOC_TYPE)?;
    Ok(html_doc_of(d))
}

/// [`html_doc_unwrap`] for a VALUE already known to be an HTML Document.
pub fn html_doc_known(rb_doc: Value) -> RawDoc {
    html_doc_of(typed_data_known_ref(rb_doc, &HTML_DOC_TYPE))
}

fn html_doc_of(d: &DocData) -> RawDoc {
    /* An lxb_html_document_t leads with its lxb_dom_document_t, so this is a
     * downcast to the embedded base, not a reinterpretation. */
    // SAFETY: `d` is the data of a live HTML Document, whose handle it owns.
    unsafe { RawDoc::from_ptr((*d.parsed).html_doc().cast()).expect("live document") }
}

/// The parsed handle behind any Document. `Err(TypeError)` for a non-Document.
pub fn doc_parsed(rb_doc: Value) -> Result<*mut Parsed, Error> {
    let d: &DocData = typed_data_ref(rb_doc, &DOC_TYPE)?;
    Ok(d.parsed)
}

/// [`doc_parsed`] for a VALUE already known to be a Document - a node's
/// keepalive Document, or the receiver of a Document method.
pub fn doc_parsed_known(rb_doc: Value) -> *mut Parsed {
    typed_data_known_ref::<DocData>(rb_doc, &DOC_TYPE).parsed
}

/// Run `f` over the parsed handle behind a Document.
///
/// The `&mut Parsed` does not escape `f`, so the raw pointer stays in this
/// layer and no alias can outlive the call. `f` must not run Ruby that could
/// re-enter this document (the readers' closures copy, they do not call back).
pub fn with_parsed<R>(rb_doc: Value, f: impl FnOnce(&mut Parsed) -> R) -> Result<R, Error> {
    let p = doc_parsed(rb_doc)?;
    // SAFETY: under the GVL, and the borrow is confined to `f`.
    Ok(unsafe { f(&mut *p) })
}

/// [`with_parsed`] for a VALUE already known to be a Document.
pub fn with_parsed_known<R>(rb_doc: Value, f: impl FnOnce(&mut Parsed) -> R) -> R {
    let p = doc_parsed_known(rb_doc);
    // SAFETY: as `with_parsed`.
    unsafe { f(&mut *p) }
}

/// The element that owns `attr` through the attr->owner index.
///
/// `Err` when the index cannot be built (out of memory) - distinct from a node
/// the index does not know, which is `Ok(None)`.
pub fn attribute_owner<'a>(rb_doc: Value, attr: RawNode) -> Result<Option<HtmlNode<'a>>, Error> {
    with_parsed(rb_doc, |p| match p.dom_index() {
        None => Err(Error::new(
            EXC_ERROR.exception(),
            "could not build the attribute index (out of memory)",
        )),
        // SAFETY: an owner the live index answers is a live node of this document.
        Some(i) => Ok(i.owner_of(attr).map(|o| unsafe { o.as_node() })),
    })?
}

/// The 1-based source line for `node`, or 0 when unknown.
pub fn node_line(rb_doc: Value, node: RawNode) -> usize {
    with_parsed_known(rb_doc, |p| {
        // SAFETY: `node` is a live node of this document.
        unsafe { p.node_line(node.as_ptr() as *const _) }
    })
}

/// The kind-AGNOSTIC raw node pointer (the base type, so HTML or XML), as an
/// opaque `*mut c_void`. Only for the few sites where the representation is
/// irrelevant (identity comparison) or already guaranteed by an external
/// same-document check (the XPath context node).
///
/// The Document branch is kind-aware: an XML Document resolves to its arena's
/// document node, an HTML one to Lexbor's.
pub fn node_raw(rb_node: Value) -> Result<*mut c_void, Error> {
    if rb_node.is_kind_of(CLASS_DOCUMENT.class()) {
        let parsed = doc_parsed(rb_node)?;
        // SAFETY: a Document's handle lives as long as the Document, and an XML
        // arena's document node is read, not written.
        unsafe {
            if (*parsed).is_xml() {
                let xdoc = parsed_xml_doc(parsed);
                return Ok(if xdoc.is_null() {
                    core::ptr::null_mut()
                } else {
                    (*xdoc).doc_node().to_token() as *mut c_void
                });
            }
        }
        return Ok(html_doc_unwrap(rb_node)?.as_ptr());
    }
    /* TypeError for a non-node, as TypedData_Get_Struct raised. */
    let nd: &NodeData = typed_data_ref(rb_node, &NODE_DATA_TYPE)?;
    Ok(nd.node)
}

/// Which representation a wrapped node is, by its TypedData type - the robust
/// discriminator, not the Ruby class. A Document, a NodeSet or any non-node is
/// `NODE_KIND_OTHER`.
pub fn node_kind(v: VALUE) -> c_int {
    if kind_of(v, &HTML_NODE_TYPE) {
        return NODE_KIND_HTML;
    }
    if kind_of(v, &XML_NODE_TYPE) {
        return NODE_KIND_XML;
    }
    NODE_KIND_OTHER
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
pub fn node_identity(rb_node: Value) -> Result<usize, Error> {
    Ok(node_raw(rb_node)? as usize)
}

/// The keepalive Document of any node, or the Document itself.
/// `Err(TypeError)` for a non-node.
pub fn keepalive_document(rb_node: Value) -> Result<Value, Error> {
    if rb_node.is_kind_of(CLASS_DOCUMENT.class()) {
        return Ok(rb_node);
    }
    let nd: &NodeData = typed_data_ref(rb_node, &NODE_DATA_TYPE)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    Ok(unsafe { value(nd.document) })
}

/* ------------------------------------------------------------------ *
 * the HTML node front door                                           *
 * ------------------------------------------------------------------ */

/// Wrap a live HTML node handle into its `Makiri::HTML::*` leaf.
///
/// A DOCUMENT node maps back onto the Ruby Document rather than getting a
/// second wrapper; a node type with no specific leaf (entity/notation, which
/// Lexbor's HTML parser does not produce) falls back to `Makiri::HTML::Node`.
pub fn wrap_html_node(node: RawNode, document: Value) -> Value {
    /* SAFETY: a `RawNode` is live - the safe constructors are `From<HtmlNode>`
     * and `From<Building*>`, and the only raw one, `from_ptr`, is unsafe. */
    let handle = unsafe { node.as_node() };
    let node_type = handle.node_type();
    if node_type == TYPE_DOCUMENT {
        return document;
    }

    let klass = match node_type {
        TYPE_ELEMENT => CLASS_HTML_ELEMENT.raw(),
        TYPE_ATTRIBUTE => CLASS_HTML_ATTR.raw(),
        TYPE_TEXT => CLASS_HTML_TEXT.raw(),
        TYPE_COMMENT => CLASS_HTML_COMMENT.raw(),
        TYPE_CDATA => CLASS_HTML_CDATA_SECTION.raw(),
        TYPE_PI => CLASS_HTML_PROCESSING_INSTRUCTION.raw(),
        TYPE_DOCTYPE => CLASS_HTML_DOCUMENT_TYPE.raw(),
        TYPE_FRAGMENT => CLASS_HTML_DOCUMENT_FRAGMENT.raw(),
        _ => CLASS_HTML_NODE.raw(),
    };

    /* The Document is stored after the wrap: see `wrap_zeroed`. */
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    unsafe {
        value(crate::bridge::ruby::wrap_zeroed::<NodeData>(
            klass,
            HTML_NODE_TYPE.as_ptr(),
            |nd| nd.node = node.as_ptr(),
            |nd| nd.document = document.as_raw(),
        ))
    }
}

/// The HTML node handle behind an HTML node or HTML Document.
///
/// `Err(TypeError)` for an XML node or Document: the typed-data check is against
/// [`HTML_NODE_TYPE`], which an XML node (wrapped under `XML_NODE_TYPE`) does
/// not satisfy.
pub fn html_node_unwrap(rb_node: Value) -> Result<RawNode, Error> {
    if rb_node.is_kind_of(CLASS_DOCUMENT.class()) {
        if rb_node.is_kind_of(CLASS_XML_DOCUMENT.class()) {
            return Err(Error::new(
                magnus::Ruby::get()
                    .expect("under the GVL")
                    .exception_type_error(),
                "expected an HTML node, got a Makiri::XML::Document",
            ));
        }
        return Ok(html_doc_unwrap(rb_node)?.into());
    }
    let nd: &NodeData = typed_data_ref(rb_node, &HTML_NODE_TYPE)?;
    RawNode::from_ptr(nd.node).ok_or_else(uninitialized)
}

/// [`html_node_unwrap`], under the name the reader modules use.
pub fn unwrap(v: Value) -> Result<RawNode, Error> {
    html_node_unwrap(v)
}

fn uninitialized() -> Error {
    Error::new(
        magnus::Ruby::get()
            .expect("under the GVL")
            .exception_type_error(),
        "uninitialized HTML node",
    )
}

/// A method receiver already checked to be an HTML node or HTML Document.
#[derive(Clone, Copy)]
pub struct HtmlSelf {
    pub value: Value,
    raw: RawNode,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl magnus::TryConvert for HtmlSelf {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let raw = unwrap(value)?;
        let document = keepalive_document(value)?;
        Ok(HtmlSelf {
            value,
            raw,
            document,
        })
    }
}

impl HtmlSelf {
    /// The receiver's node handle, for the length of this method call.
    ///
    /// The receiver is a method argument, which Ruby keeps reachable for the
    /// call and which keeps its document alive; the borrow of `self` ends the
    /// handle with the call.
    #[inline]
    pub fn node(&self) -> HtmlNode<'_> {
        // SAFETY: the receiver keeps the node's document alive for this call.
        unsafe { self.raw.as_node() }
    }

    /// The receiver's node as the boundary handle, for the mutators.
    #[inline]
    pub fn raw(&self) -> RawNode {
        self.raw
    }
}

/// An HTML node argument, for the length of the borrow of `v`.
///
/// `Err(TypeError)` for anything that is not an HTML node or HTML Document.
pub fn arg_node(v: &Value) -> Result<HtmlNode<'_>, Error> {
    // SAFETY: `v` is a method argument, which keeps its node's document alive.
    Ok(unsafe { unwrap(*v)?.as_node() })
}

/// [`wrap_html_node`] for an optional handle: nil for None.
pub fn wrap_node(node: Option<HtmlNode<'_>>, document: Value) -> Value {
    match node {
        Some(n) => wrap_html_node(RawNode::from(n), document),
        None => nil(),
    }
}

/// Wrap a boundary handle under its Document.
pub fn wrap(node: RawNode, document: Value) -> Value {
    wrap_html_node(node, document)
}

/// The keepalive Document of a node, from the kind-agnostic accessor.
pub fn node_document(v: Value) -> Result<Value, Error> {
    keepalive_document(v)
}

/* ------------------------------------------------------------------ *
 * the XML node front door                                            *
 * ------------------------------------------------------------------ */

/// Wrap an arena node token into its `Makiri::XML::*` leaf.
///
/// An invalid token becomes nil, and the DOCUMENT node maps back onto the Ruby
/// Document rather than getting a second wrapper, so the arena has exactly one
/// owner. The token resolves through `document`'s arena, where a stale or
/// foreign id reads as no node.
pub fn wrap_xml_node(node: *mut core::ffi::c_void, document: Value) -> Value {
    let id = NodeId::from_token(node as usize);
    if id.is_invalid() {
        return nil();
    }
    let xdoc = doc_of(document);
    /* An HTML Document has no arena: refuse it rather than read through null. */
    assert!(
        !xdoc.is_null(),
        "an XML node wrapped under a Document with no XML arena"
    );
    // SAFETY: `xdoc` is a live XML document; the id is read through it.
    let ty = unsafe { (*xdoc).type_(id) };
    if ty == Some(NodeType::Document) {
        return document;
    }
    let klass = match ty {
        Some(NodeType::Element) => CLASS_XML_ELEMENT.raw(),
        Some(NodeType::Attribute) => CLASS_XML_ATTR.raw(),
        Some(NodeType::Text) => CLASS_XML_TEXT.raw(),
        Some(NodeType::CData) => CLASS_XML_CDATA_SECTION.raw(),
        Some(NodeType::Comment) => CLASS_XML_COMMENT.raw(),
        Some(NodeType::Pi) => CLASS_XML_PROCESSING_INSTRUCTION.raw(),
        Some(NodeType::Doctype) => CLASS_XML_DOCUMENT_TYPE.raw(),
        Some(NodeType::Fragment) => CLASS_XML_DOCUMENT_FRAGMENT.raw(),
        _ => CLASS_XML_NODE.raw(),
    };

    /* The Document is stored after the wrap: see `wrap_zeroed`. */
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    unsafe {
        value(crate::bridge::ruby::wrap_zeroed::<NodeData>(
            klass,
            XML_NODE_TYPE.as_ptr(),
            |nd| nd.node = node,
            |nd| nd.document = document.as_raw(),
        ))
    }
}

/// The arena node token behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node.
pub fn xml_node_unwrap(rb_self: Value) -> Result<*mut core::ffi::c_void, Error> {
    if rb_self.is_kind_of(CLASS_XML_DOCUMENT.class()) {
        let parsed = doc_parsed(rb_self)?;
        // SAFETY: the handle of a live XML Document, and the arena it owns.
        let node = unsafe { (*parsed_xml_doc(parsed)).doc_node() };
        return Ok(node.to_token() as *mut core::ffi::c_void);
    }
    let nd: &NodeData = typed_data_ref(rb_self, &XML_NODE_TYPE)?;
    Ok(nd.node)
}

/// The XML arena behind a Document VALUE.
pub fn doc_of(document: Value) -> *mut XmlDoc {
    // SAFETY: `doc_parsed_known` hands back the live handle of that Document.
    unsafe { parsed_xml_doc(doc_parsed_known(document)) }
}

/// The XML arena behind a Document or node VALUE, borrowed.
///
/// Every XML node reader holds its receiver's Document, which is what keeps the
/// arena alive; `document` must be an XML Document (the Document wrappers are
/// distinct Ruby types, so an HTML one cannot reach here).
pub fn xml_doc_ref<'a>(document: Value) -> &'a XmlDoc {
    // SAFETY: an XML Document's `DocData` owns the arena, which outlives this
    // borrow.
    unsafe { &*doc_of(document) }
}

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node
/// at the type boundary, like [`xml_node_unwrap`].
pub fn xml_node_document(rb_self: Value) -> Result<Value, Error> {
    if rb_self.is_kind_of(CLASS_XML_DOCUMENT.class()) {
        return Ok(rb_self);
    }
    let nd: &NodeData = typed_data_ref(rb_self, &XML_NODE_TYPE)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    Ok(unsafe { value(nd.document) })
}
