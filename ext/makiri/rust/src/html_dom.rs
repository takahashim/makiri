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

#[cfg(not(feature = "arena-dom"))]
mod chosen {
    /// Copies between an HTML document and an XML one.
    pub use crate::lexbor::adapter::cross_import;
    /// The typed node, element, attribute and document handles, their edit and
    /// build clearances, and the name ids.
    pub use crate::lexbor::adapter::html;
    /// The parsed document handle, its indexes, and the document parse.
    pub use crate::lexbor::adapter::post_parse;
    /// A DOM step that could not get the memory it needed.
    pub use crate::lexbor::adapter::AdapterOom;
    /// Fragment parsing and importing.
    pub use crate::lexbor::fragment;
    /// HTML serialization.
    pub use crate::lexbor::serialize;
    /// XPath's HTML instance of its `Dom` trait.
    pub use crate::lexbor::xpath;
}

#[cfg(feature = "arena-dom")]
mod chosen {
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
    pub use crate::lexbor::arena_fragment as fragment;

    /// The parsed document handle, its indexes, and the document parse.
    pub mod post_parse {
        pub use crate::html::dom::HtmlParsed;
        pub use crate::lexbor::arena_parse::{
            empty_html_document, parse_html, parse_with_panicking_hook, HtmlParseError,
        };
    }
}

/// The parse guard's limits and stop reasons.
pub use crate::lexbor::adapter::tree_guard;
pub use chosen::*;
