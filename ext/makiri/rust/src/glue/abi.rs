//! The C the glue still reaches across to, and the small conveniences for
//! reaching it.
//!
//! While the port is partial this is a two-way boundary: `Init_makiri` owns the
//! class and module `VALUE`s and `ruby_node.c` owns the node wrappers' TypedData
//! types, so a ported feature reads both from C. As more of the glue moves,
//! entries leave this file rather than accumulate in it.

#![allow(unsafe_code)]

use core::ffi::c_void;

use magnus::rb_sys::AsRawValue;
use magnus::{ExceptionClass, RModule, Value};
use rb_sys::VALUE;

use crate::init::RbConst;

/// `mkr_node_data_t` - what a node wrapper holds: the node pointer plus the
/// keepalive Document. Declared here because both `glue::node` (which owns the
/// TypedData) and `glue::xml_node` (which mints XML wrappers) write it.
pub struct NodeData {
    /// `mkr_raw_node_t *` - representation-opaque; read it only through a
    /// kind-checked accessor.
    pub node: *mut c_void,
    pub document: VALUE,
}

/// An `lxb_dom_node_t`, opaque.
///
/// The glue never reads Lexbor's layout: every field it needs has an exported
/// accessor (`lxb_dom_node_type_noi` and the rest). That keeps this layer out of
/// the pinned-dependency layout problem that the XPath HTML backend has to
/// cross-check at load - there is nothing here to get wrong.
/// Now that `build.rs` generates Lexbor's layout, this IS that layout rather
/// than a second, opaque view of it. Modules that only pass the pointer along
/// are unaffected; the ones that read a field (glue::doc) get the real one, and
/// there is only one definition to be wrong.
pub type LxbNode = crate::lexbor_abi::lxb_dom_node_t;
/// `lxb_dom_document_t`. Shared vocabulary: both the Document wrapper and the
/// fragment pipeline pass it around.
pub type LxbDoc = crate::lexbor_abi::lxb_dom_document_t;

/* Every Lexbor constant below comes from the generated bindings, none is
 * transcribed. The names are re-exported here rather than used through
 * `lexbor_abi` at the call sites only because these particular ones are spelled
 * this way throughout the glue; `lexbor_abi::consts` is where a NEW one goes. */
pub const LXB_STATUS_OK: u32 = crate::lexbor_abi::lexbor_status_t_LXB_STATUS_OK;
pub const LXB_STATUS_ERROR_MEMORY_ALLOCATION: u32 =
    crate::lexbor_abi::lexbor_status_t_LXB_STATUS_ERROR_MEMORY_ALLOCATION;

pub const LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT: u32 =
    crate::lexbor_abi::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
pub const LXB_DOM_NODE_TYPE_ELEMENT: u32 =
    crate::lexbor_abi::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
pub const LXB_DOM_NODE_TYPE_DOCUMENT_TYPE: u32 =
    crate::lexbor_abi::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;

pub const LXB_HTML_SERIALIZE_OPT_UNDEF: u32 =
    crate::lexbor_abi::lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF;

/* ------------------------------------------------------------------ *
 * The Ruby-string views, declared HERE, once                         *
 * ------------------------------------------------------------------ */

/* These were defined five times between them, and not always as the same C
 * type: `BorrowedText` meant the ANCHORED three-field `mkr_ruby_borrowed_text_t`
 * in four files and the unanchored two-field `mkr_verified_text_t` in a fifth.
 * Two distinct C types under one Rust name is worse than a duplicate - it is how
 * a caller reaches for the wrong one and gets a layout that happens to compile.
 * So the names below say which type they are, and the unanchored views live in
 * `crate::text` (`VerifiedText`, `BorrowedText`) rather than gaining another
 * alias. */

/// The anchored Ruby-String views. Defined in [`crate::bridge::string`], beside
/// the functions that check a String and mint one - this layer only passes them
/// on to the glue modules that name them.
pub use crate::bridge::string::{RubyBytes, RubyData, RubyText};

/* Every symbol the glue shares with C is declared HERE, once.
 *
 * It used to be declared wherever it was needed, and that let two modules give
 * one symbol different types - `wrap_xml_node` as `*mut c_void` in one and
 * `*mut mkr_xml_node_t` in another. Rust rejects that, but only in a build where
 * both modules are present, so every single-feature CI leg passed and only the
 * "everything" leg failed. One declaration removes the possibility rather than
 * relying on that leg to notice.
 *
 * Node pointers cross this boundary as `c_void`: at the boundary a node IS
 * representation-opaque (the C calls it `mkr_raw_node_t`), and each caller casts
 * to the representation it has already established. */
pub use crate::bridge::string::ruby_bytes_view;
pub use crate::bridge::string::ruby_copy_bytes;
pub use crate::bridge::string::ruby_str_from_borrowed;
pub use crate::bridge::string::ruby_str_from_slices;
pub use crate::bridge::string::ruby_str_from_utf8;
pub use crate::bridge::string::ruby_str_known_valid_utf8;
pub use crate::bridge::string::ruby_to_utf8;
pub use crate::bridge::string::ruby_verified_text;
pub use crate::bridge::string::verify_text;
pub use crate::dom_adapter::post_parse::lxb_document_bytes;
pub use crate::glue::doc::doc_parsed;
pub use crate::glue::doc::html_doc_unwrap;
pub use crate::glue::html_node::html_node_unwrap;
pub use crate::glue::html_node::wrap_html_node;
pub use crate::glue::node::keepalive_document;
pub use crate::glue::node::node_raw;
pub use crate::glue::node_set::node_set_new;
pub use crate::glue::node_set::node_set_push;
pub use crate::glue::xml_node::wrap_xml_node;
pub use crate::glue::xml_node::xml_node_unwrap;
pub use crate::init::CLASS_DOCUMENT;
pub use crate::init::CLASS_DOCUMENT_FRAGMENT;
pub use crate::init::CLASS_HTML_DOCUMENT;
pub use crate::init::CLASS_NODE;
pub use crate::init::CLASS_NODE_SET;
pub use crate::init::CLASS_XML_DOCUMENT;
pub use crate::init::CLASS_XML_DOCUMENT_FRAGMENT;
pub use crate::init::EXC_CSS_SYNTAX_ERROR;
pub use crate::init::EXC_ERROR;
pub use crate::init::EXC_XML_LIMIT_EXCEEDED;
pub use crate::init::EXC_XML_SYNTAX_ERROR;
pub use crate::init::MOD_HTML_NODE_METHODS;
pub use crate::init::MOD_LEXBOR;
pub use crate::init::MOD_XML;
pub use crate::init::MOD_XML_NODE_METHODS;

/// The XML arena behind a parsed handle, or null for an HTML one.
///
/// # Safety
/// `p` must be a live handle.
pub unsafe fn parsed_xml_doc(
    p: *mut crate::dom_adapter::post_parse::Parsed,
) -> *mut crate::xml::model::Document {
    (*p).xml_doc()
}

/// Lexbor's `lxb_inline` accessors, through the `_noi` twins it exports. They
/// live in `lexbor_abi` - the one place in the crate that hand-declares a Lexbor
/// function, because bindgen cannot generate an inline one - and are re-exported
/// here so this module stays the single import for the glue layer.
pub use crate::lexbor_abi::{
    lxb_dom_attr_value_noi, lxb_dom_document_destroy_text_noi, lxb_dom_document_type_public_id_noi,
    lxb_dom_document_type_system_id_noi, lxb_dom_element_first_attribute_noi,
    lxb_dom_element_next_attribute_noi, lxb_dom_node_type_noi,
    lxb_dom_processing_instruction_target_noi,
};

/// The generated Lexbor readers the glue calls, likewise re-exported so a glue
/// file imports one module.
pub use crate::lexbor_abi::{
    lxb_dom_attr_local_name, lxb_dom_attr_qualified_name, lxb_dom_document_root,
    lxb_dom_element_get_attribute, lxb_dom_element_has_attribute, lxb_dom_element_local_name,
    lxb_dom_element_qualified_name, lxb_dom_element_tag_name, lxb_dom_node_name,
    lxb_dom_node_text_content, lxb_ns_by_id, LxbAttr, LxbElement,
};

/// The `Makiri::HTML::NodeMethods` module every HTML node leaf includes.
pub fn html_node_methods() -> RModule {
    MOD_HTML_NODE_METHODS.module()
}

/// Is `v` an instance of `klass`?
pub fn is_kind_of(v: Value, klass: &RbConst) -> bool {
    // SAFETY: `v` is a live value and `klass` one of `init`'s classes, which
    // `rb_obj_is_kind_of` accepts without raising.
    unsafe { rb_sys::rb_obj_is_kind_of(v.as_raw(), klass.raw()) == rb_sys::Qtrue as VALUE }
}

pub use crate::bridge::ruby::typed_data_unprotected;

/// `Makiri::Error`.
pub fn error_class() -> ExceptionClass {
    EXC_ERROR.exception()
}

/* ------------------------------------------------------------------ *
 * Lexbor's CSS parser, declared once                                 *
 * ------------------------------------------------------------------ */

/// Opaque: neither user reads a field of it.
///
/// Three users need this parser - the selector engine, the stylesheet binding,
/// and the CSS lowering, which is Ruby-free and so needs it without magnus. The
/// declaration lives in `lexbor_abi` and all three re-export it from there.
/// Giving one C symbol two Rust types is the failure this file exists to
/// prevent; it has happened twice (wrap_xml_node, and again while the
/// stylesheet binding was written). Both escaped until an "everything" build
/// compiled the two definitions together - which every build now is, so a
/// second definition is a build error rather than something a feature
/// combination has to go looking for.
pub use crate::lexbor_abi::{
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    CssParser,
};

/* ------------------------------------------------------------------ *
 * rb_data_type_t in a static                                         *
 * ------------------------------------------------------------------ */

pub use crate::bridge::ruby::DataType;
