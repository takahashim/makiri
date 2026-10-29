//! The Ruby wrappers of Makiri's nodes and Documents: the structs behind
//! them, their TypedData types and GC hooks, the one way a Document gets its
//! parsed handle ([`new_html_document`] / [`new_xml_document`]), and the representation-agnostic
//! accessors every node method starts from.
//!
//! Which representation a wrapper holds is decided here, by its TypedData
//! type; the HTML and XML front doors built on it are `bridge::html` and
//! `bridge::xml`.

mod accounting;
mod document;
mod edit;
mod node;

pub use accounting::*;
pub use document::*;
pub use edit::*;
pub use node::*;
