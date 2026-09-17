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

use crate::bridge::ruby::{typed_data_known_ref, typed_data_ref, value, DataType, VALUE};
use crate::bridge::typed::{data_type, kind_of, Hooks, Marker};
use crate::init::CLASS_DOCUMENT;
use crate::lexbor::adapter::html::RawDoc;
use crate::lexbor::adapter::post_parse::Parsed;
use crate::xml::model::Doc as XmlDoc;

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
