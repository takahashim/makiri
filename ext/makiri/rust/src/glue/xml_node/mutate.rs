//! The XML node's mutators and the Document factories (glue/ruby_xml_node.c).
//!
//! The implementations live in the Ruby <-> XML-arena seam
//! ([`crate::bridge::xml`]), which owns the token/id conversion and the unsafe
//! arena calls; this module re-exports them so `init_xml_node` registers the
//! same entry points. See that module for the detach/insert/index rules.

#![forbid(unsafe_code)]

pub use crate::bridge::xml::{
    add_child, after, aset, before, clone_node, create_cdata, create_comment, create_document_type,
    create_element, create_loose_dom_element, create_pi, create_text_node, delete, import_node,
    lshift, remove, remove_attribute_ns, replace, set_attribute_ns, set_content, set_name,
    xml_mut_check,
};
pub use crate::lexbor::adapter::cross_import::cross_html_to_xml;
pub use crate::xml::api::xml_clone_node;
pub use crate::xml::api::xml_copy_node;
pub use crate::xml::api::xml_import_subtree;
