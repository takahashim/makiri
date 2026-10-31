//! XML parser, arena, mutation primitives, and element-name index.
//!
//! A `Document` owns its nodes and byte store. Nodes are `NodeId` values and
//! fields refer to the byte store through `Span`s, so the XML engine contains
//! no raw pointers.

#![forbid(unsafe_code)]

pub mod model;
pub use model::*;

pub mod arena;
pub mod attr_key;
pub mod chars;
pub mod css;
#[cfg(test)]
mod css_tests;
pub mod dom_name;
pub mod encoding_names;
pub mod encoding_sniff;
pub mod index;
pub mod mutate;
pub mod ns_scope;
pub mod qname;
#[cfg(test)]
mod selftest;
pub mod serialize;
pub mod tree;

/// The XPath engine's XML backend (`Dom` for a borrowed `Document`).
pub mod xpath;
