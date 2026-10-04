//! The errors the DOM's insertion rules raise, worded once for HTML and XML.
//!
//! Both representations keep a refusal's reason as a
//! [`PreInsertError`](crate::dom_rules::PreInsertError) all the way here, so
//! the same broken rule reads the same in either - XML used to fold every rule
//! but two into one sentence.

#![forbid(unsafe_code)]

use crate::bridge::ruby::makiri_error;
use crate::dom_rules::{Hierarchy as H, PreInsertError, Violation};
use magnus::Error;

/// The `Makiri::Error` an insertion `e` refused raises.
pub fn pre_insert_error(e: PreInsertError) -> Error {
    makiri_error(message(e))
}

fn message(e: PreInsertError) -> &'static str {
    match e {
        PreInsertError::NoParent { replacing: true } => "cannot replace a node with no parent",
        PreInsertError::NoParent { replacing: false } => {
            "cannot add a sibling to a node with no parent"
        }
        /* Unreachable through either representation's insertion, which takes
         * the reference child from the parent it names; worded all the same. */
        PreInsertError::Rule(Violation::NotFound) => {
            "the reference node is not a child of the parent"
        }
        PreInsertError::Rule(Violation::HierarchyRequest(h)) => match h {
            H::ParentNotContainer => {
                "only a document, a document fragment or an element can have children"
            }
            H::Ancestor => "cannot insert a node into its own subtree",
            H::AttributeNode => "an attribute node cannot be inserted into the tree",
            H::DocumentNode => "a document node cannot be inserted into the tree",
            H::UnsupportedNode => "this kind of node cannot be inserted into the tree",
            H::DoctypeParent => "a doctype node can only be a child of the document",
            H::DuplicateDoctype => "the document already has a doctype",
            H::DoctypeAfterElement | H::ElementBeforeDoctype => {
                "a doctype must precede the document element"
            }
            H::SecondDocumentElement => "the document already has a root element",
            H::TextUnderDocument => "text cannot be a child of the document",
        },
    }
}
