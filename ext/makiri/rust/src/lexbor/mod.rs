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

#[cfg(test)]
mod tests;

/// Lexbor's generated layout and constants, plus the wrappers that own a raw
/// Lexbor object. The one module allowed to hold `lxb_*` bindings.
pub mod abi;
pub mod adapter;
/// Lexbor's serializer callback, shared by every serializer below.
pub mod chunks;
/// Keeps `:lexbor-contains()` from the CSS parser: every occurrence is
/// renamed before the parser sees the text, found on Lexbor's own CSS tokens.
pub mod contains_guard;
/// The HTML CSS matcher behind `Node#css`/`#at_css`/`#matches?`
/// (`glue::html_node::css`, via `selector_cache`): Lexbor's `lxb_selectors_*`
/// control flow in safe Rust, over the typed HTML adapter, on an explicit
/// heap work stack rather than native recursion. Parsing is `css_parser`'s.
pub mod css_match;
/// Lexbor's CSS syntax tokenizer, run on its own for `contains_guard`.
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
/// The Lexbor CSS stylesheet parser and its raw callback traversal.
pub mod stylesheet;

/// The XPath engine's HTML backend (`Dom` for a Lexbor document).
pub mod xpath;

/// What the CSS users share: the owner of a Lexbor object, the GVL cell, and
/// the selector parser wired to its arena.
pub(crate) mod css_engine;

/// The process-global Lexbor CSS selector parser (selector parsing only).
pub mod css_parser;
