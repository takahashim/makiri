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

/// Fragment parsing into the arena DOM through Lexbor's tree builder.
pub mod arena_fragment;
/// Parsing into the arena DOM through Lexbor's tree builder.
pub mod arena_parse;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod conformance_tests;

#[cfg(test)]
mod res_tests;

#[cfg(test)]
mod arena_tests;

#[cfg(test)]
mod tree_port_tests;

#[cfg(test)]
mod selector_port_tests;

/// The ported tree builder's output against Lexbor's, as comparable dumps.
#[cfg(any(test, feature = "lexbor-reference"))]
pub mod tree_diff;

/// The ported selector parser beside Lexbor's, for its tests and the
/// `selector_diff` fuzz target. Lexbor's parser is a process global behind
/// the GVL, which outside Ruby is `Gvl::exclusive` - so not in a Ruby build.
#[cfg(any(test, all(feature = "lexbor-reference", not(feature = "ruby"))))]
pub mod selector_diff;

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
/// The HTML CSS matcher behind `Node#css`/`#at_css`/`#matches?`
/// (`glue::html_node::css`, via `selector_cache`): Lexbor's `lxb_selectors_*`
/// control flow in safe Rust, over the typed HTML adapter, on an explicit
/// heap work stack rather than native recursion. Parsing is `css_parser`'s.
pub mod css_match;
/// Lexbor's CSS syntax tokenizer, run on its own for `contains_guard`.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub(crate) mod css_tokens;
/// HTML fragment parsing and import/fixup operations.
pub mod fragment;
/// Lexbor's allocator, padded so a small overrun past a heap block is
/// contained (hardening; the module doc).
pub mod memory;
/// The compiled-selector cache in front of `css_match`: its own
/// process-global parser/arena, separate from `css_parser`'s (shared with the
/// XML lowering), and the matcher's kept `Scratch`.
pub mod selector_cache;
/// A parsed selector written back as CSSOM text, for the stylesheet reader.
pub mod selector_text;
/// The OLD `lxb_selectors`-callback engine `css_match` replaced: compiled
/// into tests only - and into the `html_css_diff` fuzz harness, through the
/// `css-reference` feature - as the reference the differential checks
/// (`lexbor::tests::css_match`) hold `css_match` to.
#[cfg(any(test, feature = "css-reference"))]
pub mod selectors;
/// Lexbor's HTML serialization callbacks and buffer traversal.
pub mod serialize;
/// `parse_stylesheet`'s reader over the stylesheet parser port.
pub mod stylesheet;
/// Lexbor's stylesheet parser beside the port's, for the differential tests.
#[cfg(any(test, all(feature = "lexbor-reference", not(feature = "ruby"))))]
pub mod stylesheet_diff;

/// The XPath engine's HTML backend (`Dom` for a Lexbor document).
pub mod xpath;

/// What Lexbor's CSS users share: the owner of a Lexbor object and the
/// selector parser wired to its arena. Reference builds only.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub(crate) mod css_engine;

/// Lexbor's CSS selector parser, and the views over its selector lists:
/// the reference the selector and stylesheet ports are tested against.
#[cfg(any(test, feature = "lexbor-reference", feature = "css-reference"))]
pub mod css_parser;
