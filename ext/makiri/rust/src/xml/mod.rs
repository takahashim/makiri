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
//!   tree.rs    tokenizer + tree builder                               (scanning is safe;
//!                                                                     node linking unsafe)
//!   mutate.rs  mutation primitives                                   (unsafe: walks raw nodes)
//!   index.rs   element-name index                                    (unsafe: walks raw nodes)
//!   ffi.rs     the exported `mkr_xml_*` symbols                       (unsafe boundary)
//!   selftest.rs the three C self-tests, ported                       (test code)

pub mod abi;
pub use abi::*;

/* The engine itself. The layouts above stand alone, so a build that only
 * enables `xpath` still has the XML node the XPath backend walks. */
#[cfg(feature = "xml")]
pub mod arena;
#[cfg(feature = "xml")]
pub mod chars;
#[cfg(feature = "xml")]
pub mod ffi;
#[cfg(feature = "xml")]
pub mod index;
#[cfg(feature = "xml")]
pub mod mutate;
#[cfg(feature = "xml")]
pub mod qname;
#[cfg(feature = "xml")]
pub use qname::qname_from;
#[cfg(feature = "xml")]
pub mod selftest;
#[cfg(feature = "xml")]
pub mod tree;
