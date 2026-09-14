//! Compatibility re-exports for the XML glue.
//!
//! New code should import operations from `parse`, `mutate`, or `index`.

pub use crate::xml::index::{
    mkr_xml_name_index_get, mkr_xml_name_index_invalidate, mkr_xml_name_index_lookup,
};
pub use crate::xml::mutate::{
    mkr_xml_clone_node, mkr_xml_copy_node, mkr_xml_detach, mkr_xml_import_subtree,
    mkr_xml_insert_after, mkr_xml_insert_before, mkr_xml_insert_child, mkr_xml_new_chardata,
    mkr_xml_new_document_type, mkr_xml_new_element, mkr_xml_new_loose_dom_element, mkr_xml_new_pi,
    mkr_xml_remove, mkr_xml_remove_attribute, mkr_xml_remove_attribute_ns, mkr_xml_rename,
    mkr_xml_replace_node, mkr_xml_replace_with_fragment, mkr_xml_set_attribute,
    mkr_xml_set_attribute_ns, mkr_xml_set_content,
};
pub use crate::xml::parse::{
    mkr_xml_doc_destroy, mkr_xml_doc_memsize, mkr_xml_doc_new, mkr_xml_parse, mkr_xml_parse_ex,
    mkr_xml_parse_fragment, mkr_xml_preorder_next,
};
