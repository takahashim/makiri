//! The Makiri XML reader / arena / mutators, ported from
//! ext/makiri/xml/*.c behind the SAME C ABI (symbol names, struct layouts,
//! status codes), so glue/, dom_adapter/ and the XPath XML backend link against
//! it unchanged.
//!
//! Layering (the point of the spike is to measure how much of the engine can
//! be safe code when the tree is a C-layout, pointer-linked arena):
//!
//!   abi.rs     the node / document layouts and status codes          (no unsafe)
//!   chars.rs   pure byte/codepoint primitives + reference expansion  (no unsafe)
//!   qname.rs   QName splitting / xmlns detection                      (no unsafe)
//!   arena.rs   the append-only arena and node allocation              (unsafe: raw memory)
//!   tree.rs    tokenizer + tree builder                               (no unsafe code;
//!                                                                     raw nodes via ParserArena)
//!   mutate.rs  mutation primitives                                   (unsafe: walks raw nodes)
//!   index.rs   element-name index                                    (no unsafe)
//!   ffi.rs     the exported `mkr_xml_*` symbols                       (unsafe boundary)
//!   selftest.rs the three C self-tests, ported                       (test code)

pub mod abi;
pub use abi::*;

/* The engine itself. The layouts above stand alone, so a build that only
 * enables `xpath` still has the XML node the XPath backend walks. */
pub mod arena;
pub mod chars;
pub mod ffi;
pub mod index;
pub mod mutate;
pub mod qname;
pub use qname::qname_from;
pub mod selftest;
pub mod tree;

#[cfg(kani)]
mod verify;
