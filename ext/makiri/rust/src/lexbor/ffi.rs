//! Narrow re-exports of Lexbor ABI items used by higher-level facades.
//!
//! This is the only route by which non-`lexbor` modules may name an opaque
//! Lexbor handle or call an ABI function.  Layout reads remain in `adapter`.

pub type LxbNode = crate::lexbor_abi::lxb_dom_node_t;
pub type LxbDoc = crate::lexbor_abi::lxb_dom_document_t;

pub use crate::lexbor_abi::consts::{
    STATUS_ERROR_MEMORY_ALLOCATION as LXB_STATUS_ERROR_MEMORY_ALLOCATION,
    STATUS_OK as LXB_STATUS_OK,
};
pub const LXB_HTML_SERIALIZE_OPT_UNDEF: u32 =
    crate::lexbor_abi::lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF;
pub const NODE_KIND_XML: i32 = crate::lexbor_abi::parsed::NODE_KIND_XML as i32;

pub use crate::lexbor_abi::{
    lxb_css_parser_clean, lxb_css_parser_create, lxb_css_parser_destroy, lxb_css_parser_init,
    lxb_dom_attr_local_name, lxb_dom_attr_qualified_name, lxb_dom_attr_value_noi,
    lxb_dom_document_destroy_text_noi, lxb_dom_document_root,
    lxb_dom_document_type_public_id_noi, lxb_dom_document_type_system_id_noi,
    lxb_dom_element_first_attribute_noi, lxb_dom_element_get_attribute,
    lxb_dom_element_has_attribute, lxb_dom_element_local_name,
    lxb_dom_element_next_attribute_noi, lxb_dom_element_qualified_name,
    lxb_dom_element_tag_name, lxb_dom_node_name, lxb_dom_node_text_content,
    lxb_dom_node_type_noi, lxb_dom_processing_instruction_target_noi, lxb_ns_by_id,
    CssParser, LxbAttr, LxbElement,
};
