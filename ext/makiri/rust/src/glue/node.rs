//! The shared, representation-neutral node core (glue/ruby_node.c).
//!
//! HTML (Lexbor) and XML (custom-arena) nodes are two representations of one
//! Ruby-facing Node. This file owns what is common to both: the TypedData types
//! that tell the two wrappers apart, their GC functions, and the kind-agnostic
//! accessors used for identity and document lookup. Each representation's own
//! wrap/unwrap and reader methods stay where they are (ruby_html_node.c,
//! ruby_xml_node.c).
//!
//! # Why there is no magnus here
//!
//! This file defines no Ruby method. It is a library the other glue files call,
//! and the three functions that *do* end up as Ruby methods (`==`/`eql?`,
//! `hash`, `pointer_id`) are bound by C with `rb_define_method`, which means
//! they must keep the C calling convention. magnus enters where a method is
//! defined; nothing here defines one. So this port is the same shape as the
//! XPath one - identical C ABI in, identical C ABI out - and it uses rb-sys
//! directly.
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
//! raises TypeError, and vice versa. `node_data_type` is the shared base both
//! derive from, so the kind-agnostic accessors below accept either. This is the
//! single source of HTML/XML node-pointer safety - there is deliberately no
//! "return an lxb_dom_node_t for any node" unwrap.

/* Every function here takes `VALUE`s its caller holds rooted. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use rb_sys::{
    rb_data_type_t, rb_gc_mark, rb_obj_is_kind_of, rb_typeddata_is_kind_of, rb_ull2inum,
    ruby_xfree, VALUE,
};

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

#[allow(non_upper_case_globals)]
pub static node_data_type: DataType = node_type(c"Makiri::Node".as_ptr(), core::ptr::null());

#[allow(non_upper_case_globals)]
pub static html_node_type: DataType =
    node_type(c"Makiri::HTML::Node".as_ptr(), node_data_type.as_ptr());

#[allow(non_upper_case_globals)]
pub static xml_node_type: DataType =
    node_type(c"Makiri::XML::Node".as_ptr(), node_data_type.as_ptr());

/// The base type as the raw pointer the Ruby API wants.
#[inline]
fn base_type() -> *const rb_data_type_t {
    node_data_type.as_ptr()
}

/* ------------------------------------------------------------------ */
/* kind-agnostic accessors (identity / document)                      */
/* ------------------------------------------------------------------ */

/// `DocKind`.
const DOC_XML: u32 = 1;

/// `NodeKind`.
const NODE_KIND_OTHER: c_int = 0;
const NODE_KIND_HTML: c_int = 1;
const NODE_KIND_XML: c_int = 2;

use super::abi::{doc_parsed, mkr_cDocument, mkr_cNode, parsed_xml_doc, DataType};

pub use crate::dom_adapter::post_parse::parsed_kind;

#[inline]
unsafe fn is_kind_of(v: VALUE, klass: VALUE) -> bool {
    rb_obj_is_kind_of(v, klass) == rb_sys::Qtrue as VALUE
}

/// The kind-AGNOSTIC raw node pointer (the base type, so HTML or XML), as an
/// opaque `*mut c_void` - dereferencing it takes an explicit cast, so it cannot
/// be mistaken for a typed pointer. Only for the few sites where the
/// representation is irrelevant (identity comparison) or already guaranteed by
/// an external same-document check (the XPath context node).
///
/// The Document branch is kind-aware: an XML Document resolves to its arena's
/// document node, an HTML one to Lexbor's.
pub unsafe fn node_raw(rb_node: VALUE) -> Result<*mut c_void, magnus::Error> {
    if is_kind_of(rb_node, mkr_cDocument) {
        let parsed = doc_parsed(rb_node)?;
        if parsed_kind(parsed) == DOC_XML {
            let xdoc = parsed_xml_doc(parsed) as *mut XmlDoc;
            return Ok(if xdoc.is_null() {
                core::ptr::null_mut()
            } else {
                (*xdoc).doc_node().to_token() as *mut c_void
            });
        }
        return Ok(super::abi::html_doc_unwrap(rb_node)? as *mut c_void);
    }
    /* TypeError for a non-node, as TypedData_Get_Struct raised. */
    let nd = crate::bridge::ruby::typed_data(rb_node, base_type())? as *mut NodeData;
    Ok((*nd).node)
}

/// Which representation a wrapped node is, by its TypedData type - the robust
/// discriminator, not the Ruby class. A Document, a NodeSet or any non-node is
/// `NODE_KIND_OTHER`. The cross-kind `Document#import_node` entries use this
/// to route a node to the same-representation copy or the translator.
pub unsafe extern "C" fn node_kind(v: VALUE) -> c_int {
    if rb_typeddata_is_kind_of(
        v,
        &html_node_type as *const DataType as *const rb_data_type_t,
    ) != 0
    {
        return NODE_KIND_HTML;
    }
    if rb_typeddata_is_kind_of(
        v,
        &xml_node_type as *const DataType as *const rb_data_type_t,
    ) != 0
    {
        return NODE_KIND_XML;
    }
    NODE_KIND_OTHER
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
pub unsafe fn node_identity(rb_node: VALUE) -> Result<usize, magnus::Error> {
    Ok(node_raw(rb_node)? as usize)
}

/// [`node_identity`] for the C-convention identity methods below, which Ruby
/// calls directly: a failure is raised from here, where nothing is owned.
unsafe fn node_id_or_raise(rb_node: VALUE) -> usize {
    match node_identity(rb_node) {
        Ok(id) => id,
        Err(e) => crate::bridge::ruby::raise(e),
    }
}

/// The keepalive Document of any node, or the Document itself.
/// `Err(TypeError)` for a non-node.
pub unsafe fn keepalive_document(rb_node: VALUE) -> Result<VALUE, magnus::Error> {
    if is_kind_of(rb_node, mkr_cDocument) {
        return Ok(rb_node);
    }
    let nd = crate::bridge::ruby::typed_data(rb_node, base_type())? as *mut NodeData;
    Ok((*nd).document)
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
pub unsafe extern "C" fn node_equals(self_: VALUE, other: VALUE) -> VALUE {
    if !is_kind_of(other, mkr_cNode) {
        return rb_sys::Qfalse as VALUE;
    }
    if node_id_or_raise(self_) == node_id_or_raise(other) {
        rb_sys::Qtrue as VALUE
    } else {
        rb_sys::Qfalse as VALUE
    }
}

/// Nokogiri-compatible identity: the underlying node pointer as an Integer.
/// Stable for the node's lifetime and unique among currently-live nodes; a
/// freed-then-reallocated node may reuse an address (the same caveat as
/// `Nokogiri::XML::Node#pointer_id`). `a.pointer_id == b.pointer_id` iff
/// `a.eql?(b)`.
pub unsafe extern "C" fn node_pointer_id(self_: VALUE) -> VALUE {
    rb_ull2inum(node_id_or_raise(self_) as core::ffi::c_ulonglong)
}

/// A stable hash from the node pointer, so `a == b` implies `a.hash == b.hash`
/// even across separately-created wrappers. Shares the pointer value with
/// `#pointer_id`.
pub unsafe extern "C" fn node_hash(self_: VALUE) -> VALUE {
    node_pointer_id(self_)
}
