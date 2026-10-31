//! The Ruby wrappers of Makiri's nodes and Documents: the structs behind
//! them, their TypedData types and GC hooks, the one way a Document gets its
//! parsed handle ([`new_html_document`] / [`new_xml_document`]), the
//! representation-agnostic accessors every node method starts from, and what
//! a Document keeps for its nodes: the node table (`node_table`), the edit
//! versions (`edit`) and the child-position memo (`child_list`).
//!
//! Which representation a wrapper holds is decided here, by its TypedData
//! type; the HTML and XML front doors built on it are `bridge::html` and
//! `bridge::xml`.

mod accounting;
mod child_list;
mod document;
mod edit;
mod ivar;
mod node;
mod node_table;

pub use accounting::*;
pub use child_list::*;
pub use document::*;
pub use edit::*;
pub(in crate::bridge) use ivar::*;
pub use node::*;
pub(in crate::bridge) use node_table::*;
