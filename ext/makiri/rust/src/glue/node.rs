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
//! Rust does, from here on. The three `rb_data_type_t` are plain data, so
//! exporting them under the same symbols lets every C file that still wraps or
//! unwraps a node (`TypedData_Make_Struct`, `TypedData_Get_Struct`,
//! `rb_typeddata_is_kind_of`) keep working against the declarations already in
//! glue.h, unchanged. It is the same seam the XPath front end uses when a Rust
//! parser builds C AST nodes through the C allocator.
//!
//! HTML and XML nodes share the `mkr_node_data_t` layout and the same GC
//! functions but are wrapped under DISTINCT types, so the representation is
//! checked by Ruby's own type machinery: an HTML accessor handed an XML node
//! raises TypeError, and vice versa. `mkr_node_type` is the shared base both
//! derive from, so the kind-agnostic accessors below accept either. This is the
//! single source of HTML/XML node-pointer safety - there is deliberately no
//! "return an lxb_dom_node_t for any node" unwrap.

/* Every function here takes the `VALUE`s its C caller already holds; the
 * contract is the one at the declaration in glue.h. */
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use rb_sys::{
    rb_check_typeddata, rb_data_type_t, rb_gc_mark, rb_obj_is_kind_of, rb_typeddata_is_kind_of,
    rb_ull2inum, ruby_xfree, VALUE,
};

use crate::xml::abi::Doc as XmlDoc;

/// `mkr_node_data_t` - the node pointer plus the keepalive Document reference.
///
/// The node itself is owned by the document's arena (Lexbor's or the XML one),
/// so the wrapper holds only a pointer; the `document` VALUE is what keeps that
/// arena alive, and marking it is this file's whole GC job.
#[repr(C)]
struct NodeData {
    /// `mkr_raw_node_t *` - representation-opaque. Read it only through a
    /// kind-checked accessor.
    node: *mut c_void,
    document: VALUE,
}

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

/// A `rb_data_type_t` that can live in a `static`.
///
/// `rb_data_type_t` holds raw pointers, so it is not `Sync`; the C original is
/// a `const` at file scope and is equally shared. `repr(transparent)` keeps the
/// exported symbol's layout exactly `rb_data_type_t`, which is what the C
/// `extern` declarations in glue.h expect.
#[repr(transparent)]
pub struct DataType(rb_data_type_t);

// SAFETY: the contents are set once at compile time and never mutated. Ruby
// reads them from whichever thread holds the GVL.
unsafe impl Sync for DataType {}

/// Build one of the three types. `parent` is NULL for the base.
const fn data_type(name: *const c_char, parent: *const rb_data_type_t) -> DataType {
    DataType(rb_data_type_t {
        wrap_struct_name: name,
        function: rb_sys::rb_data_type_struct__bindgen_ty_1 {
            dmark: Some(node_gc_mark),
            dfree: Some(node_gc_free),
            dsize: Some(node_memsize),
            dcompact: None,
            reserved: [core::ptr::null_mut(); 1],
        },
        parent,
        data: core::ptr::null_mut(),
        flags: rb_sys::rbimpl_typeddata_flags::RUBY_TYPED_FREE_IMMEDIATELY as VALUE,
    })
}

#[no_mangle]
pub static mkr_node_type: DataType = data_type(c"Makiri::Node".as_ptr(), core::ptr::null());

#[no_mangle]
pub static mkr_html_node_type: DataType = data_type(
    c"Makiri::HTML::Node".as_ptr(),
    &mkr_node_type as *const DataType as *const rb_data_type_t,
);

#[no_mangle]
pub static mkr_xml_node_type: DataType = data_type(
    c"Makiri::XML::Node".as_ptr(),
    &mkr_node_type as *const DataType as *const rb_data_type_t,
);

/// The base type as the raw pointer the Ruby API wants.
#[inline]
fn base_type() -> *const rb_data_type_t {
    &mkr_node_type as *const DataType as *const rb_data_type_t
}

/* ------------------------------------------------------------------ */
/* kind-agnostic accessors (identity / document)                      */
/* ------------------------------------------------------------------ */

/// `mkr_doc_kind_t`.
const MKR_DOC_XML: c_int = 1;

/// `mkr_node_kind_t`.
const MKR_NODE_KIND_OTHER: c_int = 0;
const MKR_NODE_KIND_HTML: c_int = 1;
const MKR_NODE_KIND_XML: c_int = 2;

use super::abi::{mkr_cDocument, mkr_cNode, mkr_doc_parsed, mkr_parsed_xml_doc};

extern "C" {
    fn mkr_parsed_kind(p: *const c_void) -> c_int;
    fn mkr_html_doc_unwrap(rb_doc: VALUE) -> *mut c_void;
}

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
#[no_mangle]
pub unsafe extern "C" fn mkr_node_raw(rb_node: VALUE) -> *mut c_void {
    if is_kind_of(rb_node, mkr_cDocument) {
        let parsed = mkr_doc_parsed(rb_node);
        if mkr_parsed_kind(parsed) == MKR_DOC_XML {
            let xdoc = mkr_parsed_xml_doc(parsed) as *mut XmlDoc;
            return if xdoc.is_null() {
                core::ptr::null_mut()
            } else {
                (*xdoc).doc_node as *mut c_void
            };
        }
        return mkr_html_doc_unwrap(rb_node);
    }
    /* Raises TypeError for a non-node, as TypedData_Get_Struct did. Nothing in
     * this frame needs dropping, so the longjmp is safe here (glue/mod.rs). */
    let nd = rb_check_typeddata(rb_node, base_type()) as *mut NodeData;
    (*nd).node
}

/// Which representation a wrapped node is, by its TypedData type - the robust
/// discriminator, not the Ruby class. A Document, a NodeSet or any non-node is
/// `MKR_NODE_KIND_OTHER`. The cross-kind `Document#import_node` entries use this
/// to route a node to the same-representation copy or the translator.
#[no_mangle]
pub unsafe extern "C" fn mkr_node_kind(v: VALUE) -> c_int {
    if rb_typeddata_is_kind_of(v, &mkr_html_node_type as *const DataType as *const rb_data_type_t)
        != 0
    {
        return MKR_NODE_KIND_HTML;
    }
    if rb_typeddata_is_kind_of(v, &mkr_xml_node_type as *const DataType as *const rb_data_type_t)
        != 0
    {
        return MKR_NODE_KIND_XML;
    }
    MKR_NODE_KIND_OTHER
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
#[no_mangle]
pub unsafe extern "C" fn mkr_node_id(rb_node: VALUE) -> usize {
    mkr_node_raw(rb_node) as usize
}

#[no_mangle]
pub unsafe extern "C" fn mkr_node_document(rb_node: VALUE) -> VALUE {
    if is_kind_of(rb_node, mkr_cDocument) {
        return rb_node;
    }
    let nd = rb_check_typeddata(rb_node, base_type()) as *mut NodeData;
    (*nd).document
}

/* ------------------------------------------------------------------ */
/* identity (representation-neutral)                                  */
/* ------------------------------------------------------------------ */
/* These depend only on mkr_node_id, which never dereferences a node, so they
 * are identical for HTML and XML and live here rather than once per
 * representation. Both NodeMethods modules bind their ==/eql?/hash/pointer_id
 * to them. */

/// Pointer identity: equal iff both wrappers resolve to the same node pointer,
/// so an HTML node is never equal to an XML one.
#[no_mangle]
pub unsafe extern "C" fn mkr_node_equals(self_: VALUE, other: VALUE) -> VALUE {
    if !is_kind_of(other, mkr_cNode) {
        return rb_sys::Qfalse as VALUE;
    }
    if mkr_node_id(self_) == mkr_node_id(other) {
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
#[no_mangle]
pub unsafe extern "C" fn mkr_node_pointer_id(self_: VALUE) -> VALUE {
    rb_ull2inum(mkr_node_id(self_) as core::ffi::c_ulonglong)
}

/// A stable hash from the node pointer, so `a == b` implies `a.hash == b.hash`
/// even across separately-created wrappers. Shares the pointer value with
/// `#pointer_id`.
#[no_mangle]
pub unsafe extern "C" fn mkr_node_hash(self_: VALUE) -> VALUE {
    mkr_node_pointer_id(self_)
}
