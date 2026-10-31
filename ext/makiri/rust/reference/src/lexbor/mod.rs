//! The sole owner of the vendored Lexbor FFI boundary.
//!
//! Raw `lxb_*` bindings, generated layouts, and C callbacks stay in `abi` and
//! below this module. Higher layers use the typed handles exported by `adapter`.
//!
//! It is a crate of its own (`makiri_lexbor`), the reference the engine's
//! ports are held to, and depends on the engine alone: nothing here can name
//! Ruby, and nothing that ships depends on this.

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod conformance_tests;

#[cfg(test)]
mod res_tests;

#[cfg(test)]
pub(crate) mod tree_port_tests;

#[cfg(test)]
mod selector_port_tests;

/// The ported tree builder's output against Lexbor's, as comparable dumps.
#[cfg(any(test, feature = "lexbor-reference"))]
pub mod tree_diff;

/// The ported selector parser beside Lexbor's, for its tests and the
/// `selector_diff` fuzz target. Lexbor's parser is a process global behind
/// a `Gvl::exclusive` cell.
#[cfg(any(test, feature = "lexbor-reference"))]
pub mod selector_diff;

/// A Lexbor document beside its arena copy, node for node: how the CSS
/// matcher, which takes arena nodes, is held to Lexbor's own engine.
#[cfg(any(test, feature = "css-reference"))]
pub mod twin;

/// Lexbor's generated layout and constants, plus the wrappers that own a raw
/// Lexbor object. The one module allowed to hold `lxb_*` bindings.
pub mod abi;
pub mod adapter;
/// Lexbor's serializer callback, shared by every serializer below.
pub mod chunks;
/// Keeps `:lexbor-contains()` from Lexbor's CSS parser: every occurrence is
/// renamed before the parser sees the text, found on Lexbor's own CSS tokens.
/// Only the reference builds parse with Lexbor's CSS parser any more.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub mod contains_guard;
/// Lexbor's CSS syntax tokenizer, run on its own for `contains_guard`.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub(crate) mod css_tokens;
/// HTML fragment parsing and import/fixup operations.
pub mod fragment;
/// The OLD `lxb_selectors`-callback engine `css_match` replaced: compiled
/// into tests only - and into the `html_css_diff` fuzz harness, through the
/// `css-reference` feature - as the reference the differential checks
/// (`lexbor::tests::css_match`) hold `css_match` to.
#[cfg(any(test, feature = "css-reference"))]
pub mod selectors;
/// Lexbor's HTML serialization callbacks and buffer traversal.
pub mod serialize;
/// Lexbor's stylesheet parser beside the port's, for the differential tests.
#[cfg(any(test, feature = "lexbor-reference"))]
pub mod stylesheet_diff;

/// What Lexbor's CSS users share: the owner of a Lexbor object and the
/// selector parser wired to its arena. Reference builds only.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub(crate) mod css_engine;

/// Lexbor's CSS selector parser, and the views over its selector lists:
/// the reference the selector and stylesheet ports are tested against.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub mod css_parser;
