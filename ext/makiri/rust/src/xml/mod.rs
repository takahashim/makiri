//! XML parser, arena, mutation primitives, and element-name index.
//!
//! A `Document` owns its nodes and byte store. Nodes are `NodeId` values and
//! fields refer to the byte store through `Span`s, so the XML engine contains
//! no raw pointers.

pub mod abi;
pub use abi::*;

pub mod api;
pub mod arena;
pub mod chars;
pub mod index;
pub(crate) mod mutate;
pub mod qname;
#[cfg(feature = "ruby")]
pub mod selftest;
pub mod tree;

#[cfg(kani)]
mod verify;
