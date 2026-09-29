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
/// An input restriction on `:lexbor-contains()`, applied before the CSS parser
/// sees the text, decided on Lexbor's own CSS tokens.
pub mod contains_guard;
/// Lexbor's CSS syntax tokenizer, run on its own for `contains_guard`.
pub(crate) mod css_tokens;
/// HTML fragment parsing and import/fixup operations.
pub mod fragment;
/// This module's compiled-selector cache - its own process-global
/// parser/arena, separate from `css_parser`'s (shared with XML lowering) and
/// from the OLD `selectors` engine's below.
pub(crate) mod selector_cache;
/// The CSS matcher (B)-as-port
/// (notes/css_selectors_crate_migration_plan.ja.md §1.1) settled on: an
/// explicit heap work stack, not native recursion, structured as Lexbor's own
/// `lxb_selectors_*` state machine is - over the typed HTML adapter, reusing
/// `css_parser`'s existing selector AST reader. Wired into
/// `Node#css`/`#at_css`/`#matches?` (`glue::html_node::css`) via
/// `selector_cache`.
pub mod selector_port;
/// The OLD `lxb_selectors`-callback engine `selector_port` replaced for HTML
/// query. No longer on `Node#css`'s path; kept as the differential-testing
/// reference (`lexbor::tests::selector_port_spike`) until that confidence is
/// trusted enough to remove it
/// (`notes/css_selectors_crate_migration_plan.ja.md` Phase 3).
// The OLD `lxb_selectors`-backed engine: the differential tests' reference
// for `selector_port`, which is what ships.
#[cfg(test)]
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
