//! makiri - the HTML5 parser, XPath 1.0 engine and XML reader behind the
//! `makiri` gem, as one crate.
//!
//! Three layers, and the feature flags name the two optional dependencies
//! rather than any step of the C port that produced this code:
//!
//!   engine   `xml`, `xpath`, `cbuf`, `cutf8`, `falloc` - no Ruby, no Lexbor.
//!            This is what Kani proves and what cargo-fuzz drives.
//!   lexbor   `lexbor` (its `abi` and DOM facade), `css`, and the XPath HTML
//!            instance - everything that reads the vendored Lexbor DOM.
//!   ruby     `bridge`, `glue`, `init` - the magnus boundary and `Init_makiri`
//!            (`ruby`, on by default; it implies `lexbor`).
//!
//! No SYMBOL here carries the C ABI's `mkr_` prefix any more: the crate exports only
//! `Init_makiri` (and `ruby_abi_version`), so every item is named as Rust.
//! Lexbor's own names stay `lxb_*`.

#![deny(unsafe_code)]
// A panic unwinds into Ruby's `fatal` (or `Makiri::InternalError` at an
// `entry`), which kills the calling thread - a broken invariant, never an
// answer. `Cargo.toml`'s clippy lints therefore refuse a
// new `unwrap()` or `panic!` in the shipped crate; a site that genuinely wants
// one carries an `#[allow]` saying why. Test code is exempt: there a panic IS
// the failure report.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

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

/// Fallible allocation. Every engine heap allocation goes through here (the
/// glue's Ruby-side storage uses Ruby's xmalloc instead), so that `rake oom` can
/// fail it and so that failure raises instead of aborting the host process.
pub mod falloc;

/// `Buf`, the capped byte buffer more than one subsystem writes into.
pub mod cbuf;

/// Catching a panic at a C boundary and re-raising it past one, so a panic in a
/// Lexbor or Ruby callback becomes an exception rather than an abort.
pub mod caught;

/// Holding the GVL as a value: the token the process-global CSS engines ask
/// for, and the cell that keeps them.
#[cfg(feature = "lexbor")]
pub mod gvl;

/// The CSS selector front end: lowers a Lexbor-parsed selector list into the
/// XPath AST. Ruby-free, like the engine it feeds; Lexbor keeps the parser.
#[cfg(feature = "lexbor")]
pub mod css;

/// The shared UTF-8 primitives. Unconditional, like `falloc`
/// and `cbuf`: the XML and XPath layers use the strict decoder whatever the
/// feature set.
pub mod cutf8;

/// The HTML input sanitiser: invalid UTF-8 becomes U+FFFD, per WHATWG
/// byte-stream decoding. Ruby-free and Lexbor-free - it is `cbuf` and `cutf8`
/// only - so it sits with the engine even though only the HTML parse path calls
/// it.
pub mod utf8_input;

/// `VerifiedText`: a borrowed engine input, valid UTF-8 with no NUL.
/// Unconditional: the engine, the DOM adapter and the glue all pass it.
pub mod text;

/// Bounds shared across layers (`NODE_SET_MAX`). Unconditional and Ruby-free.
pub mod limits;

/// The engine error vocabulary - the kind, the message buffer and the sink -
/// shared by XPath and the CSS lowering that feeds it. Unconditional and
/// Ruby-free.
pub mod engine_error;

/// The one pointer hash, and the pointer-keyed tables the per-document indexes
/// are built on: `PtrTable` (sized once) and `PtrMap` (grows). Below every layer, so none reaches into
/// another's internals for them.
pub mod ptr_table;

/// The opaque, kind-tagged node token the engine passes around and the bridge
/// mints. Unconditional: the engine, the backends and the bridge all use it.
pub mod token;

/// The DOM node type every layer reads: the XPath engine, the HTML adapter
/// and the Ruby class table. Unconditional, like `token`.
pub mod node_type;

/// The Ruby boundary - the only part of the crate that depends on magnus.
#[cfg(feature = "ruby")]
pub mod bridge;
#[cfg(feature = "ruby")]
pub mod glue;

/// The vendored Lexbor boundary. This owns every direct use of its C ABI;
/// `adapter` supplies the compatibility facilities Lexbor does not provide.
#[cfg(feature = "lexbor")]
pub mod lexbor;

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
