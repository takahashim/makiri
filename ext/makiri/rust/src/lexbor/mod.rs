//! The sole owner of the vendored Lexbor FFI boundary.
//!
//! Raw `lxb_*` bindings, generated layouts, and C callbacks stay below this
//! module. Higher layers use the typed handles exported by `adapter`.
//!
//! # The Ruby boundary above lexbor
//!
//! Ruby-facing entry points belong in `bridge/` (which sits above this layer),
//! not here: a `Value`-taking function that reaches `bridge::lexbor` /
//! `bridge::node_set` - both built on top of `lexbor` - makes the dependency
//! two-way. Every such wrapper (`serialize`, `stylesheet`, `fragment`,
//! `selectors`) has been moved into `bridge/`; what remains here uses only the
//! lexbor-free leaves `bridge::ruby` and `bridge::string`, so this layer is
//! never reached from below.

pub mod adapter;
pub mod ffi;
/// The Lexbor CSS stylesheet parser and its raw callback traversal.
#[cfg(feature = "ruby")]
pub mod stylesheet;
/// Lexbor's HTML serialization callbacks and buffer traversal.
#[cfg(feature = "ruby")]
pub mod serialize;
/// HTML fragment parsing and import/fixup operations.
#[cfg(feature = "ruby")]
pub mod fragment;
/// Selector traversal engine, including its Lexbor callbacks.
#[cfg(feature = "ruby")]
pub mod selectors;

/// The XPath engine's HTML backend (`Dom` for a Lexbor document).
pub mod xpath;

/// The process-global Lexbor CSS selector parser (selector parsing only).
pub mod css_parser;
