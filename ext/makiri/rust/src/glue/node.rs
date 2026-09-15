//! The shared, representation-neutral node core (glue/ruby_node.c).
//!
//! HTML (Lexbor) and XML (custom-arena) nodes are two representations of one
//! Ruby-facing Node. This file owns what is common to both: the TypedData types
//! that tell the two wrappers apart, their GC functions, and the kind-agnostic
//! accessors used for identity and document lookup. Each representation's own
//! wrap/unwrap and reader methods stay where they are (ruby_html_node.c,
//! ruby_xml_node.c).
//!
//! # Where magnus comes in
//!
//! Most of this file is a library the other glue files call, over rb-sys: the
//! TypedData types and their GC functions keep the calling convention Ruby's GC
//! uses. The identity methods (`==`/`eql?`, `hash`, `pointer_id`) are ordinary
//! magnus methods that return `Result`, bound by both NodeMethods modules from
//! this one definition, so HTML and XML answer identity with the same code.
//!
//! # Who owns the TypedData
//!
//! Rust does: the three `rb_data_type_t` are statics here, and every wrap and
//! unwrap names them - through `bridge::ruby::wrap_zeroed` and
//! `bridge::ruby::typed_data`.
//!
//! HTML and XML nodes share the `mkr_node_data_t` layout and the same GC
//! functions but are wrapped under DISTINCT types, so the representation is
//! checked by Ruby's own type machinery: an HTML accessor handed an XML node
//! raises TypeError, and vice versa. `NODE_DATA_TYPE` is the shared base both
//! derive from, so the kind-agnostic accessors below accept either. This is the
//! single source of HTML/XML node-pointer safety - there is deliberately no
//! "return an lxb_dom_node_t for any node" unwrap.

/* Every function here takes `VALUE`s its caller holds rooted. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use magnus::rb_sys::FromRawValue;
use magnus::{Integer, Ruby, Value};
use rb_sys::{rb_data_type_t, rb_gc_mark, rb_typeddata_is_kind_of, ruby_xfree, VALUE};

use crate::xml::model::Doc as XmlDoc;

/* `mkr_node_data_t` lives in `super::abi`: the node wrapper holds a node pointer
 * plus the keepalive Document, and the XML wrap path writes the same struct.
 * The arena owns the node, so the Document reference is what keeps it alive and
 * marking it is this file's whole GC job. */
use super::abi::NodeData;

/* ------------------------------------------------------------------ */
/* GC + TypedData types                                               */
/* ------------------------------------------------------------------ */

unsafe extern "C" fn node_gc_mark(ptr: *mut c_void) {
    let nd = ptr as *mut NodeData;
    rb_gc_mark((*nd).document);
}

unsafe extern "C" fn node_gc_free(ptr: *mut c_void) {
    /* The node is owned by the document arena (HTML or XML); never freed here.
     * The wrapper struct came from TypedData_Make_Struct, so it goes back to
     * Ruby's allocator. */
    ruby_xfree(ptr);
}

/// The return type is `rb_sys::size_t` rather than `usize` so the signature
/// matches whatever bindgen generated for this platform's `size_t`; they are
/// the same width, but the function-pointer type has to be identical.
unsafe extern "C" fn node_memsize(_ptr: *const c_void) -> rb_sys::size_t {
    core::mem::size_of::<NodeData>() as rb_sys::size_t
}

/// The three node types, which share their GC functions.
const fn node_type(name: *const c_char, parent: *const rb_data_type_t) -> DataType {
    DataType::new(
        name,
        parent,
        Some(node_gc_mark),
        Some(node_gc_free),
        Some(node_memsize),
    )
}

pub static NODE_DATA_TYPE: DataType = node_type(c"Makiri::Node".as_ptr(), core::ptr::null());

pub static HTML_NODE_TYPE: DataType =
    node_type(c"Makiri::HTML::Node".as_ptr(), NODE_DATA_TYPE.as_ptr());

pub static XML_NODE_TYPE: DataType =
    node_type(c"Makiri::XML::Node".as_ptr(), NODE_DATA_TYPE.as_ptr());

/* ------------------------------------------------------------------ */
/* kind-agnostic accessors (identity / document)                      */
/* ------------------------------------------------------------------ */

/// `NodeKind`.
const NODE_KIND_OTHER: c_int = 0;
const NODE_KIND_HTML: c_int = 1;
const NODE_KIND_XML: c_int = 2;

use super::abi::{doc_parsed, parsed_xml_doc, DataType, CLASS_DOCUMENT, CLASS_NODE};

/// The kind-AGNOSTIC raw node pointer (the base type, so HTML or XML), as an
/// opaque `*mut c_void` - dereferencing it takes an explicit cast, so it cannot
/// be mistaken for a typed pointer. Only for the few sites where the
/// representation is irrelevant (identity comparison) or already guaranteed by
/// an external same-document check (the XPath context node).
///
/// The Document branch is kind-aware: an XML Document resolves to its arena's
/// document node, an HTML one to Lexbor's.
pub fn node_raw(rb_node: Value) -> Result<*mut c_void, magnus::Error> {
    if crate::glue::abi::is_kind_of(rb_node, &CLASS_DOCUMENT) {
        let parsed = doc_parsed(rb_node)?;
        // SAFETY: a Document's handle lives as long as the Document, and an XML
        // arena's document node is read, not written.
        unsafe {
            if (*parsed).is_xml() {
                let xdoc = parsed_xml_doc(parsed) as *mut XmlDoc;
                return Ok(if xdoc.is_null() {
                    core::ptr::null_mut()
                } else {
                    (*xdoc).doc_node().to_token() as *mut c_void
                });
            }
        }
        return Ok(super::abi::html_doc_unwrap(rb_node)? as *mut c_void);
    }
    /* TypeError for a non-node, as TypedData_Get_Struct raised. */
    let nd = crate::bridge::ruby::typed_data(rb_node, &NODE_DATA_TYPE)? as *mut NodeData;
    // SAFETY: the data pointer of a node wrapper, which lives with `rb_node`.
    Ok(unsafe { (*nd).node })
}

/// Which representation a wrapped node is, by its TypedData type - the robust
/// discriminator, not the Ruby class. A Document, a NodeSet or any non-node is
/// `NODE_KIND_OTHER`. The cross-kind `Document#import_node` entries use this
/// to route a node to the same-representation copy or the translator.
pub unsafe extern "C" fn node_kind(v: VALUE) -> c_int {
    if rb_typeddata_is_kind_of(
        v,
        &HTML_NODE_TYPE as *const DataType as *const rb_data_type_t,
    ) != 0
    {
        return NODE_KIND_HTML;
    }
    if rb_typeddata_is_kind_of(
        v,
        &XML_NODE_TYPE as *const DataType as *const rb_data_type_t,
    ) != 0
    {
        return NODE_KIND_XML;
    }
    NODE_KIND_OTHER
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
pub fn node_identity(rb_node: Value) -> Result<usize, magnus::Error> {
    Ok(node_raw(rb_node)? as usize)
}

/// The keepalive Document of any node, or the Document itself.
/// `Err(TypeError)` for a non-node.
pub fn keepalive_document(rb_node: Value) -> Result<Value, magnus::Error> {
    if crate::glue::abi::is_kind_of(rb_node, &CLASS_DOCUMENT) {
        return Ok(rb_node);
    }
    let nd = crate::bridge::ruby::typed_data(rb_node, &NODE_DATA_TYPE)? as *mut NodeData;
    // SAFETY: the data of a node wrapper, whose Document it marks and so keeps
    // alive.
    Ok(unsafe { Value::from_raw((*nd).document) })
}

/* ------------------------------------------------------------------ */
/* identity (representation-neutral)                                  */
/* ------------------------------------------------------------------ */
/* These depend only on node_identity, which never dereferences a node, so they
 * are identical for HTML and XML and live here rather than once per
 * representation. Both NodeMethods modules bind their ==/eql?/hash/pointer_id
 * to them. */

/// Pointer identity: equal iff both wrappers resolve to the same node pointer,
/// so an HTML node is never equal to an XML one.
pub fn node_equals(rb_self: Value, other: Value) -> Result<bool, magnus::Error> {
    if !crate::glue::abi::is_kind_of(other, &CLASS_NODE) {
        return Ok(false);
    }
    Ok(node_identity(rb_self)? == node_identity(other)?)
}

/// Nokogiri-compatible identity: the underlying node pointer as an Integer.
/// Stable for the node's lifetime and unique among currently-live nodes; a
/// freed-then-reallocated node may reuse an address (the same caveat as
/// `Nokogiri::XML::Node#pointer_id`). `a.pointer_id == b.pointer_id` iff
/// `a.eql?(b)`.
pub fn node_pointer_id(ruby: &Ruby, rb_self: Value) -> Result<Integer, magnus::Error> {
    Ok(ruby.integer_from_u64(node_identity(rb_self)? as u64))
}

/// A stable hash from the node pointer, so `a == b` implies `a.hash == b.hash`
/// even across separately-created wrappers. Shares the pointer value with
/// `#pointer_id`.
pub fn node_hash(ruby: &Ruby, rb_self: Value) -> Result<Integer, magnus::Error> {
    node_pointer_id(ruby, rb_self)
}
