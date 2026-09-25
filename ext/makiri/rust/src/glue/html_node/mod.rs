//! The HTML (Lexbor) node surface: every method on `Makiri::HTML::NodeMethods`,
//! in a submodule per kind - `read`, `mutate`, `css`, `serialize`.
//!
//! The XML counterpart is `glue::xml_node`; what both share - identity by node
//! pointer - is `glue::node`, and the wrappers behind that pointer are
//! `bridge::wrapper`'s.
//!
//! # Two functions here are the HTML node's front door
//!
//! [`crate::bridge::html::wrap_html_node`] and
//! [`crate::bridge::html::html_node_unwrap`] are how every glue module wraps
//! and unwraps an HTML node; they live in [`crate::bridge::html`], the one seam
//! that knows both the Ruby wrapper and the Lexbor handle.
//!
//! # Nothing here is declared twice
//!
//! Every Lexbor accessor comes from the `lexbor` layer, which re-exports the
//! generated bindings - the `_noi` twins included. Allowlisting a name in
//! build.rs and finding no binding is what identifies an `lxb_inline` function;
//! eight of the eighteen readers this file needs turned out to be inline-only,
//! and on macOS a hand-written declaration of one of those links to nothing and
//! becomes a NULL call at run time rather than a link error.

pub mod css;

pub mod read;

pub mod serialize;

pub mod mutate;

use magnus::{method, prelude::*, Error};

use crate::init::MOD_HTML_NODE_METHODS;
/* Only the mutation half registers on the Document class. */
use crate::init::CLASS_HTML_DOCUMENT;

/* ------------------------------------------------------------------ *
 * the DOM node types                                                 *
 * ------------------------------------------------------------------ */

/// The DOM node types, under the short names this layer reads best. Defined
/// once in [`crate::lexbor::adapter::html`], which is where the generated values
/// are read - a second definition of a node type is how every HTML element
/// once became foreign (see that module).
pub mod ty {
    pub use crate::lexbor::adapter::html::{
        TYPE_ATTRIBUTE as ATTRIBUTE, TYPE_CDATA as CDATA, TYPE_COMMENT as COMMENT,
        TYPE_DOCTYPE as DOCTYPE, TYPE_DOCUMENT as DOCUMENT, TYPE_ELEMENT as ELEMENT,
        TYPE_FRAGMENT as FRAGMENT, TYPE_PI as PI, TYPE_TEXT as TEXT,
    };
}

use crate::glue::html_doc::node_clone_node;
use crate::glue::node::{node_equals, node_hash, node_pointer_id};
use crate::init::{CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT};

/* The receiver and argument handles, from the Ruby <-> Lexbor seam
 * (`bridge::html`), for the readers and for `glue::html_doc`. */
pub use crate::bridge::html::{arg_node, wrap_node, HtmlSelf};

/* ------------------------------------------------------------------ *
 * registration                                                       *
 * ------------------------------------------------------------------ */

/// The whole HTML node surface. From `Init_makiri`, after the classes exist.
pub fn init() -> Result<(), Error> {
    init_read()?;
    init_mutate()?;
    css::init_css()?;
    serialize::init_serialize()?;
    Ok(())
}

/// The readers, identity, and the DocumentType and `<template>` accessors.
fn init_read() -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.module();

    m.define_method("name", method!(read::name, 0))?;
    m.define_method("namespace_uri", method!(read::namespace_uri, 0))?;
    m.define_method("prefix", method!(read::prefix, 0))?;
    m.define_method("local_name", method!(read::local_name, 0))?;
    m.define_method("tag_name", method!(read::tag_name, 0))?;
    m.define_method("target", method!(read::pi_target, 0))?;
    m.define_method("node_type", method!(read::node_type, 0))?;
    for name in ["content", "text", "inner_text"] {
        m.define_method(name, method!(read::content, 0))?;
    }

    m.define_method("document", method!(read::get_document, 0))?;
    m.define_method("parent", method!(read::parent, 0))?;
    for name in ["next", "next_sibling"] {
        m.define_method(name, method!(read::next, 0))?;
    }
    for name in ["previous", "previous_sibling"] {
        m.define_method(name, method!(read::previous, 0))?;
    }
    m.define_method("next_element", method!(read::next_element, 0))?;
    m.define_method("previous_element", method!(read::previous_element, 0))?;

    m.define_method("child", method!(read::child, 0))?;
    m.define_method("children", method!(read::children, 0))?;
    for name in ["element_children", "elements"] {
        m.define_method(name, method!(read::element_children, 0))?;
    }
    m.define_method("first_element_child", method!(read::first_element_child, 0))?;
    m.define_method("last_element_child", method!(read::last_element_child, 0))?;
    m.define_method("ancestors", method!(read::ancestors, 0))?;

    m.define_method("[]", method!(read::aref, 1))?;
    m.define_method("key?", method!(read::has_key, 1))?;
    m.define_method("keys", method!(read::keys, 0))?;
    m.define_method("values", method!(read::values, 0))?;
    m.define_method("attribute_nodes", method!(read::attribute_nodes, 0))?;
    m.define_method(
        "attribute_by_qualified_name",
        method!(read::attribute_by_qualified_name, 1),
    )?;
    m.define_method(
        "attribute_value_by_qualified_name",
        method!(read::attribute_value_by_qualified_name, 1),
    )?;
    m.define_method("value", method!(read::value, 0))?;
    m.define_method("line", method!(read::line, 0))?;

    /* Identity is by the node pointer and shared with the XML side; document
     * order is HTML-only and lives in read.rs. */
    m.define_method("==", method!(node_equals, 1))?;
    m.define_method("eql?", method!(node_equals, 1))?;
    m.define_method("hash", method!(node_hash, 0))?;
    m.define_method("pointer_id", method!(node_pointer_id, 0))?;
    m.define_method("clone_node", method!(node_clone_node, -1))?;

    m.define_method("<=>", method!(read::spaceship, 1))?;

    /* DocumentType identifiers (WHATWG DOM names; external_id is the
     * Nokogiri-compatible alias for public_id). */
    let dt = CLASS_HTML_DOCUMENT_TYPE.class();
    for name in ["public_id", "external_id"] {
        dt.define_method(name, method!(read::doctype_public_id, 0))?;
    }
    dt.define_method("system_id", method!(read::doctype_system_id, 0))?;

    /* <template> contents (WHATWG DOM HTMLTemplateElement.content). */
    let el = CLASS_HTML_ELEMENT.class();
    el.define_method("content_fragment", method!(read::content_fragment, 0))?;
    Ok(())
}

/// The mutators and the Document factories.
fn init_mutate() -> Result<(), Error> {
    let m = MOD_HTML_NODE_METHODS.module();
    let doc = CLASS_HTML_DOCUMENT.class();

    m.define_method("add_child", method!(mutate::add_child, 1))?;
    m.define_method("<<", method!(mutate::lshift, 1))?;
    for name in ["add_previous_sibling", "before"] {
        m.define_method(name, method!(mutate::before, 1))?;
    }
    for name in ["add_next_sibling", "after"] {
        m.define_method(name, method!(mutate::after, 1))?;
    }
    for name in ["remove", "unlink"] {
        m.define_method(name, method!(mutate::remove, 0))?;
    }
    m.define_method("replace", method!(mutate::replace, 1))?;

    m.define_method("inner_html=", method!(mutate::set_inner_html, 1))?;
    m.define_method("outer_html=", method!(mutate::set_outer_html, 1))?;

    m.define_method("[]=", method!(mutate::aset, 2))?;
    m.define_method("set_attribute_ns", method!(mutate::set_attribute_ns, 3))?;
    m.define_method(
        "remove_attribute_ns",
        method!(mutate::remove_attribute_ns, 2),
    )?;
    for name in ["delete", "remove_attribute"] {
        m.define_method(name, method!(mutate::delete, 1))?;
    }
    m.define_method("content=", method!(mutate::set_content, 1))?;

    doc.define_method("create_element", method!(mutate::create_element, 1))?;
    doc.define_method(
        "create_document_type",
        method!(mutate::create_document_type, -1),
    )?;
    doc.define_method("create_text_node", method!(mutate::create_text_node, 1))?;
    doc.define_method("create_comment", method!(mutate::create_comment, 1))?;
    doc.define_method(
        "create_processing_instruction",
        method!(mutate::create_pi, 2),
    )?;
    doc.define_method(
        "create_document_fragment",
        method!(mutate::create_document_fragment, 0),
    )?;
    Ok(())
}
