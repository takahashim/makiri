//! The sole owner of the vendored Lexbor FFI boundary.
//!
//! Raw `lxb_*` bindings, generated layouts, and C callbacks stay in `abi` and
//! below this module. Higher layers use the typed handles exported by `adapter`.
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

/// Lexbor's generated layout and constants, plus the wrappers that own a raw
/// Lexbor object. The one module allowed to hold `lxb_*` bindings.
pub mod abi;
pub mod adapter;
/// HTML fragment parsing and import/fixup operations.
pub mod fragment;
/// Selector traversal engine, including its Lexbor callbacks.
pub mod selectors;
/// Lexbor's HTML serialization callbacks and buffer traversal.
pub mod serialize;
/// The Lexbor CSS stylesheet parser and its raw callback traversal.
pub mod stylesheet;

/// The XPath engine's HTML backend (`Dom` for a Lexbor document).
pub mod xpath;

/// What the CSS users share: the owner of a Lexbor object, the GVL cell, and
/// the selector parser wired to its arena.
pub(crate) mod css_engine;

/// The process-global Lexbor CSS selector parser (selector parsing only).
pub mod css_parser;
