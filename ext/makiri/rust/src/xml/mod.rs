//! The Makiri XML reader / index arena / mutators.
//!
//! The tree is an **index arena**: a `Document` owns a `Vec<Node>` and a byte
//! store, a node is a `NodeId` (index plus generation), links are
//! `Option<NodeId>`, and names/values are `(offset, len)` spans. No raw pointer
//! is part of the model, so every module below is ordinary safe Rust; the only
//! unsafe left is the FFI boundary that turns raw document handles into
//! references (`ffi.rs`, and the XPath XML backend's one-deref adapter).
//!
//! Layering:
//!
//!   abi.rs     the node / document model and status codes            (no unsafe)
//!   chars.rs   pure byte/codepoint primitives + reference expansion  (no unsafe)
//!   qname.rs   QName splitting / xmlns detection                      (no unsafe)
//!   arena.rs   the document: node/byte allocation and tree linking    (no unsafe)
//!   tree.rs    tokenizer + tree builder                               (no unsafe)
//!   mutate.rs  mutation primitives                                    (no unsafe)
//!   index.rs   element-name index                                    (no unsafe)
//!   ffi.rs     the `mkr_xml_*` boundary over raw document handles     (unsafe boundary)
//!   selftest.rs the three C self-tests, ported                       (test code)

pub mod abi;
pub use abi::*;

/* The engine itself. The layouts above stand alone, so a build that only
 * enables `xpath` still has the XML node the XPath backend walks. */
pub mod arena;
pub mod chars;
pub mod ffi;
pub mod index;
pub(crate) mod mutate;
pub mod qname;
#[cfg(feature = "ruby")]
pub mod selftest;
pub mod tree;

#[cfg(kani)]
mod verify;
