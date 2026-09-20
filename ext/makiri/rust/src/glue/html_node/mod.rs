//! The HTML (Lexbor) node representation (glue/ruby_html_node.c).
//!
//! Wrapping an `lxb_dom_node_t` into a `Makiri::HTML::*` leaf, the HTML
//! node-pointer accessor, and the reader methods that hang off
//! `Makiri::HTML::NodeMethods`. The XML counterpart is `glue::xml_node`; the
//! representation-neutral node core - the `rb_data_type_t` chain and the
//! kind-agnostic accessors - is `glue::node`.
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

use magnus::{method, prelude::*, RClass};

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

/// `init_node` - the HTML node surface.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub fn init_node() {
    let m = MOD_HTML_NODE_METHODS.module();

    m.define_method("name", method!(read::name, 0))
        .expect("#name");
    m.define_method("namespace_uri", method!(read::namespace_uri, 0))
        .expect("#namespace_uri");
    m.define_method("prefix", method!(read::prefix, 0))
        .expect("#prefix");
    m.define_method("local_name", method!(read::local_name, 0))
        .expect("#local_name");
    m.define_method("tag_name", method!(read::tag_name, 0))
        .expect("#tag_name");
    m.define_method("target", method!(read::pi_target, 0))
        .expect("#target");
    m.define_method("node_type", method!(read::node_type, 0))
        .expect("#node_type");
    for name in ["content", "text", "inner_text"] {
        m.define_method(name, method!(read::content, 0))
            .expect("#content");
    }

    m.define_method("document", method!(read::get_document, 0))
        .expect("#document");
    m.define_method("parent", method!(read::parent, 0))
        .expect("#parent");
    for name in ["next", "next_sibling"] {
        m.define_method(name, method!(read::next, 0))
            .expect("#next");
    }
    for name in ["previous", "previous_sibling"] {
        m.define_method(name, method!(read::previous, 0))
            .expect("#previous");
    }
    m.define_method("next_element", method!(read::next_element, 0))
        .expect("#next_element");
    m.define_method("previous_element", method!(read::previous_element, 0))
        .expect("#previous_element");

    m.define_method("child", method!(read::child, 0))
        .expect("#child");
    m.define_method("children", method!(read::children, 0))
        .expect("#children");
    for name in ["element_children", "elements"] {
        m.define_method(name, method!(read::element_children, 0))
            .expect("#element_children");
    }
    m.define_method("first_element_child", method!(read::first_element_child, 0))
        .expect("#first_element_child");
    m.define_method("last_element_child", method!(read::last_element_child, 0))
        .expect("#last_element_child");
    m.define_method("ancestors", method!(read::ancestors, 0))
        .expect("#ancestors");

    m.define_method("[]", method!(read::aref, 1)).expect("#[]");
    m.define_method("key?", method!(read::has_key, 1))
        .expect("#key?");
    m.define_method("keys", method!(read::keys, 0))
        .expect("#keys");
    m.define_method("values", method!(read::values, 0))
        .expect("#values");
    m.define_method("attribute_nodes", method!(read::attribute_nodes, 0))
        .expect("#attribute_nodes");
    m.define_method(
        "attribute_by_qualified_name",
        method!(read::attribute_by_qualified_name, 1),
    )
    .expect("#attribute_by_qualified_name");
    m.define_method(
        "attribute_value_by_qualified_name",
        method!(read::attribute_value_by_qualified_name, 1),
    )
    .expect("#attribute_value_by_qualified_name");
    m.define_method("value", method!(read::value, 0))
        .expect("#value");
    m.define_method("line", method!(read::line, 0))
        .expect("#line");

    /* Identity is by the node pointer and shared with the XML side; document
     * order is HTML-only and lives in read.rs. */
    m.define_method("==", method!(node_equals, 1)).expect("#==");
    m.define_method("eql?", method!(node_equals, 1))
        .expect("#eql?");
    m.define_method("hash", method!(node_hash, 0))
        .expect("#hash");
    m.define_method("pointer_id", method!(node_pointer_id, 0))
        .expect("#pointer_id");
    m.define_method("clone_node", method!(node_clone_node, -1))
        .expect("#clone_node");

    m.define_method("<=>", method!(read::spaceship, 1))
        .expect("#<=>");

    /* DocumentType identifiers (WHATWG DOM names; external_id is the
     * Nokogiri-compatible alias for public_id). */
    let dt =
        RClass::from_value(CLASS_HTML_DOCUMENT_TYPE.value()).expect("Makiri::HTML::DocumentType");
    for name in ["public_id", "external_id"] {
        dt.define_method(name, method!(read::doctype_public_id, 0))
            .expect("#public_id");
    }
    dt.define_method("system_id", method!(read::doctype_system_id, 0))
        .expect("#system_id");

    /* <template> contents (WHATWG DOM HTMLTemplateElement.content). */
    let el = RClass::from_value(CLASS_HTML_ELEMENT.value()).expect("Makiri::HTML::Element");
    el.define_method("content_fragment", method!(read::content_fragment, 0))
        .expect("#content_fragment");
}

/// `init_mutate` - the HTML node's mutators and the Document factories.
///
/// # Safety
/// From `Init_makiri`, after the classes exist.
pub fn init_mutate() {
    let m = MOD_HTML_NODE_METHODS.module();
    let doc = RClass::from_value(CLASS_HTML_DOCUMENT.value()).expect("HTML::Document");

    m.define_method("add_child", method!(mutate::add_child, 1))
        .expect("#add_child");
    m.define_method("<<", method!(mutate::lshift, 1))
        .expect("#<<");
    for name in ["add_previous_sibling", "before"] {
        m.define_method(name, method!(mutate::before, 1))
            .expect("#before");
    }
    for name in ["add_next_sibling", "after"] {
        m.define_method(name, method!(mutate::after, 1))
            .expect("#after");
    }
    for name in ["remove", "unlink"] {
        m.define_method(name, method!(mutate::remove, 0))
            .expect("#remove");
    }
    m.define_method("replace", method!(mutate::replace, 1))
        .expect("#replace");

    m.define_method("inner_html=", method!(mutate::set_inner_html, 1))
        .expect("#inner_html=");
    m.define_method("outer_html=", method!(mutate::set_outer_html, 1))
        .expect("#outer_html=");

    m.define_method("[]=", method!(mutate::aset, 2))
        .expect("#[]=");
    m.define_method("set_attribute_ns", method!(mutate::set_attribute_ns, 3))
        .expect("#set_attribute_ns");
    m.define_method(
        "remove_attribute_ns",
        method!(mutate::remove_attribute_ns, 2),
    )
    .expect("#remove_attribute_ns");
    for name in ["delete", "remove_attribute"] {
        m.define_method(name, method!(mutate::delete, 1))
            .expect("#delete");
    }
    m.define_method("content=", method!(mutate::set_content, 1))
        .expect("#content=");
    m.define_method("name=", method!(mutate::set_name, 1))
        .expect("#name=");

    doc.define_method("create_element", method!(mutate::create_element, 1))
        .expect("#create_element");
    doc.define_method(
        "create_document_type",
        method!(mutate::create_document_type, -1),
    )
    .expect("#create_document_type");
    doc.define_method("create_text_node", method!(mutate::create_text_node, 1))
        .expect("#create_text_node");
    doc.define_method("create_comment", method!(mutate::create_comment, 1))
        .expect("#create_comment");
    doc.define_method(
        "create_processing_instruction",
        method!(mutate::create_pi, 2),
    )
    .expect("#create_processing_instruction");
    doc.define_method(
        "create_document_fragment",
        method!(mutate::create_document_fragment, 0),
    )
    .expect("#create_document_fragment");
}
