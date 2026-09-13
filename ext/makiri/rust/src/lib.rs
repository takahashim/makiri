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

/// Lexbor's layout and constants, generated from its own headers by build.rs
/// and checked against the hand-written view the engine's hot paths use.
#[cfg(feature = "lexbor-abi")]
pub mod lexbor_abi;

/// Compile-time decimal parsing, for the settings that arrive as `option_env!`
/// overrides: a const context cannot call `parse`.
///
/// Two kinds of setting use it. The `verify` modules bound the input each set of
/// proofs quantifies over - and raising such a bound means raising that
/// harness's `#[kani::unwind]`, which cannot be computed because Kani wants a
/// literal. Getting it wrong fails loudly (`unwinding assertion`), not silently,
/// so the override is safe to use for an experiment without editing the default.
/// The buffer's content ceilings (`cbuf`) use it for a different reason: they
/// were `-D`-overridable C macros, and standing alone that override has to
/// arrive from the environment instead.
///
/// Hence ungated. It was `#[cfg(kani)]` while the proofs were its only caller.
pub mod kani_bounds {
    /// Decimal only; a non-digit is a compile error naming the bound.
    pub const fn parse_usize(s: &str) -> usize {
        let b = s.as_bytes();
        let mut i = 0;
        let mut n = 0usize;
        while i < b.len() {
            assert!(b[i] >= b'0' && b[i] <= b'9', "the bound must be a decimal number");
            n = n * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        n
    }
}

/// Fallible allocation. Every heap allocation in Rust code that does not
/// already go through the C allocator goes through here, so that `rake oom` can
/// fail it and so that failure raises instead of aborting the host process.
pub mod falloc;

/// `mkr_buf_t`, which more than one subsystem writes into.
pub mod cbuf;

/// The CSS selector front end (xpath/mkr_css.c): lowers a Lexbor-parsed
/// selector list into the XPath AST. Ruby-free, like the engine it feeds.
#[cfg(feature = "css-lower")]
pub mod css;

/// The shared UTF-8 primitives (core/mkr_utf8.c). Unconditional, like `falloc`
/// and `cbuf`: the XML and XPath layers use the strict decoder whatever the
/// feature set, and only the `#[no_mangle]` C entries are gated.
pub mod cutf8;

/// The XPath engine's C types. Shared with the glue, which holds an error, a
/// value and a limits pointer at the XML query entry points.
pub mod xpath_abi;

/// The Ruby boundary. Present only when a glue feature is on, because it is the
/// one part of the crate that depends on magnus.
#[cfg(feature = "glue")]
pub mod bridge;
#[cfg(feature = "glue")]
pub mod glue;

/// The Lexbor gap-fillers (ext/makiri/dom_adapter/).
#[cfg(feature = "dom-adapter")]
pub mod dom_adapter;

/// The XML node layouts (`xml::abi`) come in with either feature: the XPath
/// port's XML backend walks those nodes without needing the reader.
#[cfg(any(
    feature = "xml",
    feature = "xpath",
    feature = "glue-node",
    feature = "glue-xml",
    feature = "glue-xml-node-read",
    feature = "glue-xpath"
))]
pub mod xml;

#[cfg(feature = "xpath")]
pub mod xpath;

/// `Init_makiri` and the class hierarchy (makiri.c). Only under `standalone`,
/// where there is no C extension to own them: with the C present this module
/// would define a second `Init_makiri`, so the feature that turns it on is the
/// same one that asserts the C is gone.
#[cfg(feature = "standalone")]
pub mod init;
