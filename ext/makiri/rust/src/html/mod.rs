//! The HTML parser, ported from Lexbor to Rust.
//!
//! `docs/rewrite_parser.ja.md` is the plan. The port follows Lexbor's
//! control flow and data model module by module, so that any difference from
//! the vendored Lexbor (the reference, compared in `lexbor::conformance_tests`)
//! is a porting error unless it is listed as an intended change. Nothing here
//! calls Lexbor or Ruby.

#![forbid(unsafe_code)]

pub mod dom;
pub mod fragment;
pub mod names;
pub mod res;
pub mod tokenizer;
pub mod tree;
