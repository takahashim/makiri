//! makiri - the HTML5 parser, XPath 1.0 engine and XML reader behind the
//! `makiri` gem, as one crate.
//!
//! Three layers, and the feature flags name the two optional dependencies
//! rather than any step of the C port that produced this code:
//!
//!   engine   `xml`, `xpath`, `cbuf`, `cutf8`, `falloc` - no Ruby, no Lexbor.
//!            This is what Kani proves and what cargo-fuzz drives.
//!   lexbor   `lexbor_abi`, `css`, `dom_adapter`, and the XPath HTML instance -
//!            everything that reads the vendored Lexbor DOM (`lexbor`).
//!   ruby     `bridge`, `glue`, `init` - the magnus boundary and `Init_makiri`
//!            (`ruby`, on by default; it implies `lexbor`).
//!
//! The `mkr_` prefix on exported symbols is what the C ABI published and is kept
//! where an entry point is still reached from outside Rust: `Init_makiri`, the
//! callbacks Lexbor invokes, and the test hooks. It is not a naming rule for
//! Rust-internal items.

/// Lexbor's layout and constants, generated from its own headers by build.rs
/// and checked against the hand-written view the engine's hot paths use.
#[cfg(feature = "lexbor")]
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
            assert!(
                b[i] >= b'0' && b[i] <= b'9',
                "the bound must be a decimal number"
            );
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

/// The CSS selector front end: lowers a Lexbor-parsed selector list into the
/// XPath AST. Ruby-free, like the engine it feeds; Lexbor keeps the parser.
#[cfg(feature = "lexbor")]
pub mod css;

/// The shared UTF-8 primitives (core/mkr_utf8.c). Unconditional, like `falloc`
/// and `cbuf`: the XML and XPath layers use the strict decoder whatever the
/// feature set, and only the `#[no_mangle]` C entries are gated.
pub mod cutf8;

/// Borrowed text views and their two contracts: `VerifiedText` (no NUL, for
/// engine inputs) and `BorrowedText` (NUL permitted, for DOM data).
/// Unconditional: the engine, the DOM adapter and the glue all pass them.
pub mod text;

/// The XPath engine's C types. Shared with the glue, which holds an error, a
/// value and a limits pointer at the XML query entry points.
pub mod xpath_abi;

/// The Ruby boundary - the only part of the crate that depends on magnus.
#[cfg(feature = "ruby")]
pub mod bridge;
#[cfg(feature = "ruby")]
pub mod glue;

/// What Lexbor does not provide and we will not patch it to: the attr->owner
/// and element indices, the text index, source locations, cross-import, and the
/// input sanitiser.
#[cfg(feature = "lexbor")]
pub mod dom_adapter;

/// The XML reader and its arena. Ruby-free and Lexbor-free.
pub mod xml;

/// The XPath 1.0 engine, generic over a `Dom` trait. The XML instance needs
/// nothing else; the HTML instance is gated on `lexbor` inside the module.
pub mod xpath;

/// `Init_makiri` and the class hierarchy: the symbol Ruby looks up at require
/// time, and the one export the extension publishes.
#[cfg(feature = "ruby")]
pub mod init;

// These tests deliberately use the Ruby- and Lexbor-free feature set.  Kani
// proves bounded symbolic properties in a separate job; this module gives the
// normal Rust test runner exhaustive small-domain and boundary-value coverage
// on every representative CI build.
#[cfg(test)]
mod rust_tests;
