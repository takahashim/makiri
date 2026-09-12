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

/// Shared by the `verify` modules: the input bound each set of proofs quantifies
/// over is an `option_env!` override, and a const context cannot call `parse`.
///
/// Raising a bound also means raising that harness's `#[kani::unwind]`, which
/// cannot be computed - Kani wants a literal. Getting it wrong fails loudly
/// (`unwinding assertion`), not silently, so the override is safe to use for an
/// experiment without editing the default.
#[cfg(kani)]
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
