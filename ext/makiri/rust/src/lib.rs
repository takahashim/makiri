//! makiri_rs - Makiri subsystems ported from C, each behind the SAME C ABI as
//! the sources it replaces (symbol names, struct layouts, status codes), so the
//! rest of the extension links against either one unchanged.
//!
//! Each subsystem is a cargo feature, and extconf turns a feature on at the same
//! time as it drops the C sources that feature replaces. Nothing is shared
//! between them yet, so a build may enable either, both, or neither:
//!
//!   xml        ext/makiri/xml/*.c                  reader, arena, mutators
//!   xpath      xpath/mkr_xpath_{lex,number,parse}.c lexer, Number, parser
//!   xpath-xml  xpath/mkr_xpath_engine_xml.c         the XML engine instance
//!
//! The front end builds the C AST through the C allocator, so either evaluator
//! runs what it parses. The engine is generic over a `Dom` trait, which is the
//! type-checked form of the monomorphization the C does by including the same
//! bodies once per representation; `xpath-xml` instantiates it for the XML node,
//! and the HTML backend is the remaining step
//! (notes/rust_rewrite_plan.ja.md §7).

/// `mkr_buf_t`, which more than one subsystem writes into.
pub mod cbuf;

/// The XPath engine's C types. Shared with the glue, which holds an error, a
/// value and a limits pointer at the XML query entry points.
pub mod xpath_abi;

/// The Ruby boundary. Present only when a glue feature is on, because it is the
/// one part of the crate that depends on magnus.
#[cfg(feature = "glue")]
pub mod bridge;
#[cfg(feature = "glue")]
pub mod glue;

/// The XML node layouts (`xml::abi`) come in with either feature: the XPath
/// port's XML backend walks those nodes without needing the reader.
#[cfg(any(
    feature = "xml",
    feature = "xpath",
    feature = "glue-node",
    feature = "glue-xml"
))]
pub mod xml;

#[cfg(feature = "xpath")]
pub mod xpath;
