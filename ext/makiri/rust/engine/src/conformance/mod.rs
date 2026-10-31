//! The HTML parser conformance harness, shared by every parser the crate can
//! build.
//!
//! The parser port (`docs/rewrite_parser.ja.md`) judges a parser by the DOM it
//! builds, not by what that DOM serializes to: two trees can serialize alike
//! and still differ in a namespace, in where one text node ends and the next
//! begins, or in the document's quirks mode, and each of those changes what
//! XPath, CSS and the DOM API answer. So every parser is read into the one
//! owned [`tree::Tree`] here, and parsers are compared as trees.
//!
//! - [`tree`]: the parser-neutral tree, its exact comparison and two renderings
//!   (the html5lib dump the test data is written in, and a full one that shows
//!   everything the comparison looks at).
//! - [`dat`]: the html5lib `.dat` tree-construction format.
//! - [`tokens`]: the html5lib tokenizer tests and their token model, read
//!   with the small [`json`] reader.
//! - [`known`]: the divergences already classified against the pinned HTML
//!   Standard, shared with the Ruby runner.
//! - [`res_gen`]: the generator of `html::res`'s tree tables and entity tree,
//!   from the pinned Standard and `entities.json`.
//!
//! Nothing here knows which parser built a tree; each parser's reader lives
//! with that parser (`lexbor::conformance_tests` for Lexbor).

#![forbid(unsafe_code)]
// The engine's tests use all of it; the Lexbor reference, which builds it
// with `lexbor-compat`, only some.
#![cfg_attr(not(test), allow(dead_code))]
// A test harness, built for the Lexbor reference too (`lexbor-compat`): a
// panic in it is a failed test, there as here.
#![cfg_attr(
    not(test),
    allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)
)]
// Test-only: the harness builds owned trees and sorted lists with std's
// infallible allocation, which `clippy.toml` bans for the engine so that an
// OOM there raises. Aborting a test process on OOM is fine.
#![allow(clippy::disallowed_methods)]

pub mod dat;
pub mod json;
pub mod known;
pub mod res_gen;
pub mod tokens;
pub mod tree;

use std::path::PathBuf;

/// `spec/conformance/` in the source tree.
pub fn spec_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../spec/conformance")
}

/// The html5lib tokenizer `.test` files, fetched (pinned) by
/// `spec/conformance/parser_data.rb`.
pub fn tokenizer_dir() -> PathBuf {
    spec_dir().join("data/html5lib-tests/tokenizer")
}

/// The tree-construction `.dat` files: WPT's `html/syntax/parsing/resources`,
/// fetched (pinned) by `spec/conformance/parser_data.rb`.
pub fn tree_construction_dir() -> PathBuf {
    spec_dir().join("data/wpt/html/syntax/parsing/resources")
}
