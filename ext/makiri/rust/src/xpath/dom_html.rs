//! Lexbor DOM backend facade.
//!
//! The raw layout access and the `DomRaw` implementation live in
//! [`super::lexbor_abi`]. This module remains as the stable backend name used
//! by the evaluator and glue.

pub use super::lexbor_abi::Html;

use super::dom::*;
use crate::lexbor_abi as lxb;

/* The engine reads every node's type through the shared `NTYPE_*` encoding, so
 * Lexbor's enum must agree value for value; a mismatch would make an HTML walk
 * misread each node rather than fail. Checked at compile time, against the
 * generated header view, in every build that has an HTML backend. */
const _: () = {
    assert!(NTYPE_ELEMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT);
    assert!(NTYPE_ATTRIBUTE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ATTRIBUTE);
    assert!(NTYPE_TEXT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT);
    assert!(NTYPE_CDATA_SECTION == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION);
    assert!(NTYPE_ENTITY_REFERENCE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ENTITY_REFERENCE);
    assert!(NTYPE_ENTITY == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ENTITY);
    assert!(NTYPE_PI == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION);
    assert!(NTYPE_COMMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_COMMENT);
    assert!(NTYPE_DOCUMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT);
    assert!(NTYPE_DOCUMENT_TYPE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE);
    assert!(NTYPE_NOTATION == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_NOTATION);
};
