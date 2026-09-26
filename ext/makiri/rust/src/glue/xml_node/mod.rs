//! The XML node surface: the readers (`read`), namespace introspection (`ns`),
//! mutation and the document factories (`mutate`), and `#to_xml` /
//! `#canonicalize` (`serialize`), registered on `Makiri::XML::NodeMethods`.
//!
//! Nothing here touches Lexbor, and nothing restates the node layout: the arena
//! is `crate::xml`'s, reached through the seam in `crate::bridge::xml`.

#![forbid(unsafe_code)]

pub mod css;
pub mod mutate;
pub mod ns;
pub mod read;
pub mod serialize;
pub mod strings;

use magnus::{method, prelude::*, Error, RModule};

use crate::glue::node::{node_equals, node_hash, node_pointer_id};
use crate::init::{CLASS_XML_DOCUMENT, CLASS_XML_DOCUMENT_TYPE, MOD_XML_NODE_METHODS};

/* The wrapper and the receiver handle live in the Ruby <-> XML-arena seam
 * (`bridge::xml`). */
pub use crate::bridge::xml::{wrap_xml_node as wrap, XmlSelf};

fn node_methods() -> Result<RModule, Error> {
    MOD_XML_NODE_METHODS.defined()
}

/// The whole XML node surface. From `Init_makiri`, after the classes exist.
pub fn init() -> Result<(), Error> {
    init_read()?;
    init_ns()?;
    init_mutate()?;
    css::init_xml_css()?;
    serialize::init_xml_node_serialize()?;
    Ok(())
}

/// The readers - names, content, navigation, attributes - and identity.
fn init_read() -> Result<(), Error> {
    let m = node_methods()?;

    m.define_method("name", method!(read::name, 0))?;
    m.define_method("local_name", method!(read::local_name, 0))?;
    m.define_method("prefix", method!(read::prefix, 0))?;
    m.define_method("namespace_uri", method!(read::namespace_uri, 0))?;
    m.define_method("tag_name", method!(read::tag_name, 0))?;
    m.define_method("target", method!(read::pi_target, 0))?;
    m.define_method("node_type", method!(read::node_type, 0))?;

    /* `value` too: an attribute's content IS its value. */
    for name in ["content", "text", "inner_text", "value"] {
        m.define_method(name, method!(read::content, 0))?;
    }

    m.define_method("document", method!(read::get_document, 0))?;
    m.define_method("parent", method!(read::parent, 0))?;
    m.define_method("<=>", method!(read::spaceship, 1))?;
    for name in ["next", "next_sibling"] {
        m.define_method(name, method!(read::next, 0))?;
    }
    for name in ["previous", "previous_sibling"] {
        m.define_method(name, method!(read::previous, 0))?;
    }
    m.define_method("next_element", method!(read::next_element, 0))?;
    m.define_method("previous_element", method!(read::previous_element, 0))?;
    m.define_method("child", method!(read::first_child, 0))?;
    m.define_method("children", method!(read::children, 0))?;
    for name in ["element_children", "elements"] {
        m.define_method(name, method!(read::element_children, 0))?;
    }
    m.define_method("first_element_child", method!(read::first_element_child, 0))?;
    m.define_method("last_element_child", method!(read::last_element_child, 0))?;

    for name in ["[]", "attribute_value_by_qualified_name"] {
        m.define_method(name, method!(read::aref, 1))?;
    }
    m.define_method("keys", method!(read::keys, 0))?;
    m.define_method("values", method!(read::values, 0))?;
    m.define_method("attribute_nodes", method!(read::attribute_nodes, 0))?;
    m.define_method(
        "attribute_by_qualified_name",
        method!(read::attribute_by_qualified_name, 1),
    )?;

    /* Node identity by the underlying pointer, so #path, NodeSet dedup, Set and
     * Hash all work - the same contract HTML nodes have, from the same code. */
    for name in ["==", "eql?"] {
        m.define_method(name, method!(node_equals, 1))?;
    }
    m.define_method("hash", method!(node_hash, 0))?;
    m.define_method("pointer_id", method!(node_pointer_id, 0))?;

    /* DocumentType identifiers; #public_id is the Nokogiri-style alias of
     * #external_id, and #name comes from the shared reader above. */
    let dt = CLASS_XML_DOCUMENT_TYPE.defined()?;
    for name in ["external_id", "public_id"] {
        dt.define_method(name, method!(read::dtd_external_id, 0))?;
    }
    dt.define_method("system_id", method!(read::dtd_system_id, 0))?;
    Ok(())
}

/// Namespace introspection. The `Makiri::XML::Namespace` it hands back is
/// defined in Ruby.
fn init_ns() -> Result<(), Error> {
    let m = node_methods()?;
    m.define_method("namespace", method!(ns::namespace, 0))?;
    m.define_method(
        "namespace_definitions",
        method!(ns::namespace_definitions, 0),
    )?;
    m.define_method("namespaces", method!(ns::namespaces, 0))?;
    m.define_method("collect_namespaces", method!(ns::collect_namespaces, 0))?;
    Ok(())
}

/// The mutators, insertion, and the Document factories.
fn init_mutate() -> Result<(), Error> {
    let m = node_methods()?;
    let doc = CLASS_XML_DOCUMENT.defined()?;

    /* In-place edits. Detach, never destroy: the primitives are
     * `crate::xml::mutate`'s. */
    for name in ["remove", "unlink"] {
        m.define_method(name, method!(mutate::remove, 0))?;
    }
    m.define_method("[]=", method!(mutate::aset, 2))?;
    for name in ["delete", "remove_attribute"] {
        m.define_method(name, method!(mutate::delete, 1))?;
    }
    m.define_method("set_attribute_ns", method!(mutate::set_attribute_ns, 3))?;
    m.define_method(
        "remove_attribute_ns",
        method!(mutate::remove_attribute_ns, 2),
    )?;
    m.define_method("content=", method!(mutate::set_content, 1))?;

    /* Building. Insertion accepts a single Makiri::XML node; one from another
     * document is deep-copied into this one. */
    m.define_method("add_child", method!(mutate::add_child, 1))?;
    m.define_method("<<", method!(mutate::lshift, 1))?;
    for name in ["add_previous_sibling", "before"] {
        m.define_method(name, method!(mutate::before, 1))?;
    }
    for name in ["add_next_sibling", "after"] {
        m.define_method(name, method!(mutate::after, 1))?;
    }
    m.define_method("replace", method!(mutate::replace, 1))?;
    m.define_method("clone_node", method!(mutate::clone_node, -1))?;

    /* Document factories. The node-class .new constructors and Document#root=
     * are pure delegations to these, defined once in the Ruby layer. */
    doc.define_method("create_element", method!(mutate::create_element, -1))?;
    doc.define_method(
        "create_loose_dom_element",
        method!(mutate::create_loose_dom_element, 4),
    )?;
    doc.define_method(
        "create_document_type",
        method!(mutate::create_document_type, -1),
    )?;
    doc.define_method("create_text_node", method!(mutate::create_text_node, 1))?;
    doc.define_method("create_comment", method!(mutate::create_comment, 1))?;
    for name in ["create_cdata", "create_cdata_node"] {
        doc.define_method(name, method!(mutate::create_cdata, 1))?;
    }
    doc.define_method(
        "create_processing_instruction",
        method!(mutate::create_pi, 2),
    )?;
    doc.define_method("import_node", method!(mutate::import_node, -1))?;
    Ok(())
}
