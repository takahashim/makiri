//! The HTML DOM the rest of the crate reads and edits, as one path.
//!
//! Everything outside the Lexbor layer that touches an HTML document - the
//! Ruby glue and bridge, XPath's HTML instance, the node-type mapping - takes
//! its types from here rather than naming `lexbor::adapter` directly. Today
//! every item is Lexbor's. The parser port (`docs/rewrite_parser.ja.md`)
//! brings a second implementation of the same API, an arena of Rust's own, and
//! this module is where a build chooses between them; the code that uses the
//! DOM does not change with the choice.
//!
//! Only re-exports live here, so the module carries no `unsafe` and no
//! decisions of its own.

#![forbid(unsafe_code)]

/// Copies between an HTML document and an XML one.
pub use crate::lexbor::adapter::cross_import;
/// The typed node, element, attribute and document handles, their edit and
/// build clearances, and the name ids.
pub use crate::lexbor::adapter::html;
/// The parsed document handle, its indexes, and the document parse.
pub use crate::lexbor::adapter::post_parse;
/// The parse guard's limits and stop reasons.
pub use crate::lexbor::adapter::tree_guard;
/// A DOM step that could not get the memory it needed.
pub use crate::lexbor::adapter::AdapterOom;
/// Fragment parsing and importing.
pub use crate::lexbor::fragment;
/// HTML serialization.
pub use crate::lexbor::serialize;
/// XPath's HTML instance of its `Dom` trait.
pub use crate::lexbor::xpath;
