//! XML parser, arena, mutation primitives, and element-name index.
//!
//! A `Document` owns its nodes and byte store. Nodes are `NodeId` values and
//! fields refer to the byte store through `Span`s, so the XML engine contains
//! no raw pointers.

#![forbid(unsafe_code)]

pub mod model;
pub use model::*;

pub mod api;
pub mod arena;
pub mod chars;
pub mod index;
pub mod mutate;
pub mod parse;
pub mod qname;
#[cfg(test)]
mod selftest;
pub mod serialize;
pub mod tree;

#[cfg(kani)]
mod verify;

/// The XPath engine's XML backend (`Dom` for a borrowed `Document`).
pub mod xpath;
