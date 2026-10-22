//! The HTML DOM the rest of the crate reads and edits, as one path.
//!
//! Everything outside the Lexbor layer that touches an HTML document - the
//! Ruby glue and bridge, XPath's HTML instance, the node-type mapping - takes
//! its types from here rather than naming `crate::html::dom` directly. It is
//! the parser port's index arena (`docs/rewrite_parser.ja.md`), parsed by the
//! ported tree builder. Lexbor's DOM (`lexbor::adapter`) is not behind it any
//! more: it is only the reference the arena is tested against.
//!
//! Only re-exports live here, so the module carries no `unsafe` and no
//! decisions of its own.

#![forbid(unsafe_code)]

/// The typed handles, the edit, and the name ids.
pub use crate::html::dom as html;
/// Copies between an HTML document and an XML one.
pub use crate::html::dom::cross_import;
/// HTML serialization.
pub use crate::html::dom::serialize;
/// XPath's HTML instance of its `Dom` trait.
pub use crate::html::dom::xpath;
/// A DOM step that could not get the memory it needed.
pub use crate::html::dom::DomOom as AdapterOom;
/// Fragment parsing and importing.
pub use crate::html::fragment;

/// The parsed document handle, its indexes, and the document parse.
pub mod post_parse {
    pub use crate::html::dom::HtmlParsed;
    pub use crate::html::tree::{
        empty_html_document, parse_html, parse_with_panicking_hook, HtmlParseError,
    };
}

/// The parse guard's limits and stop reasons.
pub mod tree_guard {
    pub use crate::html::tree::guard::{DepthLimit, GuardStop};
}
