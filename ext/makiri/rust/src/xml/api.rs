//! Compatibility re-exports for the XML glue.
//!
//! New code should import operations from `parse`, `mutate`, or `index`.

pub use crate::xml::index::{xml_name_index_get, xml_name_index_invalidate, xml_name_index_lookup};
pub use crate::xml::mutate::{
    xml_clone_node, xml_copy_node, xml_detach, xml_import_subtree, xml_insert_after,
    xml_insert_before, xml_insert_child, xml_new_chardata, xml_new_document_type, xml_new_element,
    xml_new_loose_dom_element, xml_new_pi, xml_remove, xml_remove_attribute,
    xml_remove_attribute_ns, xml_rename, xml_replace_node, xml_replace_with_fragment,
    xml_set_attribute, xml_set_attribute_ns, xml_set_content,
};
pub use crate::xml::parse::{
    xml_doc_destroy, xml_doc_memsize, xml_doc_new, xml_parse, xml_parse_ex, xml_parse_fragment,
    xml_preorder_next,
};
