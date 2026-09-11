//! makiri_rs - Makiri subsystems ported from C, each behind the SAME C ABI as
//! the sources it replaces (symbol names, struct layouts, status codes), so the
//! rest of the extension links against either one unchanged.
//!
//! Each subsystem is a cargo feature, and extconf turns a feature on at the same
//! time as it drops the C sources that feature replaces. Nothing is shared
//! between them yet, so a build may enable either, both, or neither:
//!
//!   xml    ext/makiri/xml/*.c            - reader, arena, mutators
//!   xpath  the XPath front end           - lexer, Number, parser
//!
//! The `xpath` feature covers only the front end so far. It builds the C AST
//! through the C allocator, so the C evaluator runs what it parses and the whole
//! existing suite is the differential test (notes/rust_rewrite_plan.ja.md §7).

#![allow(clippy::missing_safety_doc)]

#[cfg(feature = "xml")]
pub mod xml;

#[cfg(feature = "xpath")]
pub mod xpath;
