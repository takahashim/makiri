//! The sole owner of the vendored Lexbor FFI boundary.
//!
//! Raw `lxb_*` bindings, generated layouts, and C callbacks stay below this
//! module. Higher layers use the typed handles exported by `adapter`.
//!
//! # The Ruby boundary above lexbor
//!
//! Ruby-facing entry points belong in `bridge/` (which sits above this layer),
//! not here: a `Value`-taking function that reaches `bridge::html` /
//! `bridge::node_set` - both built on top of `lexbor` - makes the dependency
//! two-way. The cyclic importers (`serialize`, the fragment context helpers,
//! `selectors`) have been moved into `bridge/`, and the last two - the fragment
//! parser, which took a Ruby String, and the stylesheet binding, which built
//! Ruby hashes - now take bytes and return their own errors, with the Ruby half
//! in `bridge::string::HtmlSource` and `glue::stylesheet`. Nothing here names a
//! Ruby type, and `rake unsafe:boundaries` holds it to that.

pub mod adapter;
pub mod ffi;
/// HTML fragment parsing and import/fixup operations.
pub mod fragment;
/// Selector traversal engine, including its Lexbor callbacks.
#[cfg(feature = "ruby")]
pub mod selectors;
/// Lexbor's HTML serialization callbacks and buffer traversal.
#[cfg(feature = "ruby")]
pub mod serialize;
/// The Lexbor CSS stylesheet parser and its raw callback traversal.
pub mod stylesheet;

/// The XPath engine's HTML backend (`Dom` for a Lexbor document).
pub mod xpath;

/// The process-global Lexbor CSS selector parser (selector parsing only).
pub mod css_parser;
