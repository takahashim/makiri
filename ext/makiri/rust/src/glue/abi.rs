//! The C the glue still reaches across to, and the small conveniences for
//! reaching it.
//!
//! While the port is partial this is a two-way boundary: `Init_makiri` owns the
//! class and module `VALUE`s and `ruby_node.c` owns the node wrappers' TypedData
//! types, so a ported feature reads both from C. As more of the glue moves,
//! entries leave this file rather than accumulate in it.

#![forbid(unsafe_code)]


use magnus::{prelude::*, ExceptionClass, RModule, Value};

use crate::init::{RbConst, EXC_ERROR, MOD_HTML_NODE_METHODS};

/// `mkr_node_data_t` - what a node wrapper holds. Owned by the DOM seam
/// ([`crate::bridge::lexbor`]), beside the TypedData that frees and marks it.
pub use crate::bridge::lexbor::NodeData;


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
pub use crate::bridge::lexbor::{doc_parsed, html_doc_unwrap, keepalive_document, node_raw};
pub use crate::bridge::lexbor::parsed_xml_doc;
pub use crate::bridge::lexbor::{html_node_unwrap, wrap_html_node};
pub use crate::bridge::node_set::{node_set_new, node_set_push, node_set_with_fill};
pub use crate::glue::xml_node::wrap_xml_node;
pub use crate::glue::xml_node::xml_node_unwrap;

/// The `Makiri::HTML::NodeMethods` module every HTML node leaf includes.
pub fn html_node_methods() -> RModule {
    MOD_HTML_NODE_METHODS.module()
}

/// Is `v` an instance of `klass`?
pub fn is_kind_of(v: Value, klass: &RbConst) -> bool {
    v.is_kind_of(klass.class())
}

pub use crate::bridge::ruby::typed_data_unprotected;

/// `Makiri::Error`.
pub fn error_class() -> ExceptionClass {
    EXC_ERROR.exception()
}


/* ------------------------------------------------------------------ *
 * rb_data_type_t in a static                                         *
 * ------------------------------------------------------------------ */

pub use crate::bridge::ruby::DataType;
