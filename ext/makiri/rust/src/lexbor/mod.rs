//! The sole owner of the vendored Lexbor FFI boundary.
//!
//! Raw `lxb_*` bindings, generated layouts, and C callbacks stay below this
//! module. Higher layers use the typed handles exported by `adapter`.
//!
//! # The Ruby boundary above lexbor
//!
//! Ruby-facing entry points belong in `bridge/` (which sits above this layer),
//! not here: a `Value`-taking function drags in `bridge::lexbor` /
//! `bridge::node_set`, which are built on top of `lexbor`, and makes the
//! dependency two-way. `serialize.rs` and `stylesheet.rs` are already split so
//! this layer is only reached from above; `fragment.rs` and `selectors.rs` still
//! carry their Ruby wrappers and are the remaining two to move into `bridge/`.

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
