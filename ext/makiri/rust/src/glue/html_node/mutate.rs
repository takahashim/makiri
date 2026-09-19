//! The HTML node's mutators and the Document factories (glue/ruby_html_mutate.c).
//!
//! The implementations live in the Ruby <-> Lexbor seam ([`crate::bridge::html`]),
//! beside the wrapper they mutate: the structural verbs and their adopt/fragment
//! rules, the attribute and content setters, and the Document factories. This
//! module re-exports them so `init_mutate` registers the same entry points it
//! always did.

#![forbid(unsafe_code)]

pub use crate::bridge::html::{
    add_child, after, aset, before, create_comment, create_document_fragment, create_document_type,
    create_element, create_pi, create_text_node, delete, lshift, remove, remove_attribute_ns,
    replace, set_attribute_ns, set_content, set_inner_html, set_name, set_outer_html,
};
pub use crate::bridge::string::ruby_verified_data;
