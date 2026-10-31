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
//! Ruby type: the fuzz crate builds it without the `ruby` feature, where
//! naming one would not compile.

#[cfg(all(test, feature = "lexbor"))]
mod tests;

#[cfg(all(test, feature = "lexbor"))]
pub mod conformance_tests;

#[cfg(all(test, feature = "lexbor"))]
mod res_tests;

#[cfg(all(test, feature = "lexbor"))]
pub(crate) mod tree_port_tests;

#[cfg(all(test, feature = "lexbor"))]
mod selector_port_tests;

/// The ported tree builder's output against Lexbor's, as comparable dumps.
#[cfg(all(feature = "lexbor", any(test, feature = "lexbor-reference")))]
pub mod tree_diff;

/// The ported selector parser beside Lexbor's, for its tests and the
/// `selector_diff` fuzz target. Lexbor's parser is a process global behind
/// the GVL, which outside Ruby is `Gvl::exclusive` - so not in a Ruby build.
#[cfg(all(
    feature = "lexbor",
    any(test, all(feature = "lexbor-reference", not(feature = "ruby")))
))]
pub mod selector_diff;

/// A Lexbor document beside its arena copy, node for node: how the CSS
/// matcher, which takes arena nodes, is held to Lexbor's own engine.
#[cfg(all(feature = "lexbor", any(test, feature = "css-reference")))]
pub mod twin;

/// Lexbor's generated layout and constants, plus the wrappers that own a raw
/// Lexbor object. The one module allowed to hold `lxb_*` bindings.
#[cfg(feature = "lexbor")]
pub mod abi;
#[cfg(feature = "lexbor")]
pub mod adapter;
/// Lexbor's serializer callback, shared by every serializer below.
#[cfg(feature = "lexbor")]
pub mod chunks;
/// Keeps `:lexbor-contains()` from Lexbor's CSS parser: every occurrence is
/// renamed before the parser sees the text, found on Lexbor's own CSS tokens.
/// Only the reference builds parse with Lexbor's CSS parser any more.
#[cfg(all(
    feature = "lexbor",
    any(test, feature = "lexbor-reference", feature = "css-reference")
))]
pub mod contains_guard;
/// Lexbor's CSS syntax tokenizer, run on its own for `contains_guard`.
#[cfg(all(
    feature = "lexbor",
    any(test, feature = "lexbor-reference", feature = "css-reference")
))]
pub(crate) mod css_tokens;
/// HTML fragment parsing and import/fixup operations.
#[cfg(feature = "lexbor")]
pub mod fragment;
/// The OLD `lxb_selectors`-callback engine `css_match` replaced: compiled
/// into tests only - and into the `html_css_diff` fuzz harness, through the
/// `css-reference` feature - as the reference the differential checks
/// (`lexbor::tests::css_match`) hold `css_match` to.
#[cfg(all(feature = "lexbor", any(test, feature = "css-reference")))]
pub mod selectors;
/// Lexbor's HTML serialization callbacks and buffer traversal.
#[cfg(feature = "lexbor")]
pub mod serialize;
/// Lexbor's stylesheet parser beside the port's, for the differential tests.
#[cfg(all(
    feature = "lexbor",
    any(test, all(feature = "lexbor-reference", not(feature = "ruby")))
))]
pub mod stylesheet_diff;

/// What Lexbor's CSS users share: the owner of a Lexbor object and the
/// selector parser wired to its arena. Reference builds only.
#[cfg(all(
    feature = "lexbor",
    any(test, feature = "lexbor-reference", feature = "css-reference")
))]
pub(crate) mod css_engine;

/// Lexbor's CSS selector parser, and the views over its selector lists:
/// the reference the selector and stylesheet ports are tested against.
#[cfg(all(
    feature = "lexbor",
    any(test, feature = "lexbor-reference", feature = "css-reference")
))]
pub mod css_parser;
