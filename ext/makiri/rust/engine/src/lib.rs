//! makiri_engine - the HTML parser and its DOM, the XML reader, XPath 1.0 and
//! CSS selectors behind the `makiri` gem.
//!
//! It knows neither Ruby nor Lexbor: it depends on `falloc` and two Rust
//! libraries, and nothing else. The extension (`makiri_rs`) puts a Ruby surface
//! on it, and the Lexbor reference (`makiri_lexbor`) holds the ports to the
//! library they were ported from. That it is Ruby-free is this crate's
//! dependency list, not a rule a lint keeps: a Ruby value or a name of the
//! binding layer does not compile here. So a GVL-released parse and an edit's
//! closure, which call only this crate, run no Ruby.

// No `unsafe` at all. The one an engine needs - a fallible `Box<T>` - is
// `falloc`'s. The check build of `rake no_unwind` alone calls an undefined
// symbol (`no_unwind`), and only there is `unsafe` allowed.
#![cfg_attr(not(makiri_no_unwind), forbid(unsafe_code))]
#![cfg_attr(makiri_no_unwind, deny(unsafe_code))]
// A panic unwinds into `Makiri::InternalError` at the extension's method
// trampoline - a broken invariant, never an answer - so the workspace's clippy
// lints refuse a new `unwrap()` or `panic!`; a site that genuinely wants one
// carries an `#[allow]` saying why. Test code is exempt: there a panic IS the
// failure report.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

/// Compile-time decimal parsing, for the settings that arrive as `option_env!`
/// overrides: a const context cannot call `parse`.
///
/// The buffer's content ceilings (`ceiling_buf`) use it: they were
/// `-D`-overridable C macros, and standing alone that override has to arrive
/// from the environment instead. (The Kani harnesses' input bounds used it
/// too, and named it, while there were any.)
pub mod env_bounds {
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

/// `Buf`, the byte buffer with a content ceiling that more than one
/// subsystem writes into.
pub mod ceiling_buf;

/// A Document's content, reachable only through a counted borrow guard -
/// what the extension's `bridge::dom` holds each Document's parse in, against
/// Ruby's many references to one Document. Ruby-free, so the guards' promises
/// are tested here, without a Ruby.
pub mod doc_cell;

/// The bodies that must not unwind (a `Drop` run from the GC), and the build
/// that checks they cannot.
pub mod no_unwind;

/// The CSS matcher behind `Node#css`/`#at_css`/`#matches?` (through
/// `css_cache`): Lexbor's `lxb_selectors_*` control flow in safe Rust, on an
/// explicit heap work stack rather than native recursion, over the selector
/// parser's views and any DOM that implements its traits (`css_match::dom`).
pub mod css_match;

/// The compiled-selector cache in front of `css_match`, with the `Scratch`
/// each host's queries borrow: one per thread, since a query never releases
/// the GVL. It names the two hosts (`html::dom::css`, `xml::css`), so it sits
/// beside the matcher, which knows none, rather than in it.
#[cfg(feature = "html-dom")]
pub mod css_cache;

/// The shared UTF-8 primitives. Unconditional, like `ceiling_buf`: the XML and XPath
/// layers use the strict decoder whatever the feature set.
pub mod utf8_decode;

/// The HTML input sanitiser: invalid UTF-8 becomes U+FFFD, per WHATWG
/// byte-stream decoding. It is `ceiling_buf` and `utf8_decode` only, and sits here even
/// though only the HTML parse path calls it.
pub mod utf8_input;

/// The source line table (byte offset -> line).
pub mod source_lines;

/// Word-at-a-time byte tests, for the tokenizer and the line table.
pub mod swar;

/// `VerifiedText`: a borrowed engine input, valid UTF-8 with no NUL.
pub mod text;

/// Bounds shared across layers (`NODE_SET_MAX`) and the one rule every work
/// budget charges through.
pub mod limits;

/// The native-stack-exhaustion probe the engine's recursion checks consult,
/// installed by the extension at `Init_makiri` - see the module doc for why a
/// depth count alone does not bound the stack.
pub mod stack;

/// The engine error vocabulary - the kind, the message buffer and the sink -
/// of XPath's front end and evaluator.
pub mod engine_error;

/// The one pointer hash, and the pointer-keyed tables the per-document indexes
/// are built on: `PtrTable` (sized once) and `PtrMap` (grows).
pub mod ptr_table;

/// The opaque, kind-tagged node token the engine passes around and the
/// extension mints.
pub mod token;

/// The DOM node type every layer reads: the XPath engine, the HTML DOM and the
/// Ruby class table.
pub mod node_type;

/// The WHATWG DOM's pre-insertion rules, once for the HTML DOM and the XML
/// arena alike.
pub mod dom_rules;

/// Counting and indexing a child list without building it, with the
/// position memo a document keeps.
pub mod child_index;

/// The HTML DOM as the rest of the engine and the extension reach it: one
/// path, to the parser port's index arena (`html::dom`).
#[cfg(feature = "html-dom")]
pub mod html_dom;

/// The HTML parser ported from Lexbor to Rust (`docs/rewrite_parser.ja.md`).
pub mod html;

/// The CSS selector parser port.
pub mod selectors;

/// Where the parser ports follow the specifications and Lexbor does not:
/// the `fixed(Fix::...)` gate the Lexbor reference turns off.
pub mod compat;

/// The XML reader and its arena.
pub mod xml;

/// The XPath 1.0 engine, generic over a `Dom` trait, and its two instances.
pub mod xpath;

// These tests deliberately use the bare feature set. Verus proves the
// contracts at every length (`verus/`); this module gives the normal Rust test
// runner exhaustive small-domain and boundary-value coverage on every
// representative CI build.
#[cfg(test)]
mod rust_tests;

/// The HTML parser conformance harness: a parser-neutral DOM tree, the
/// html5lib formats, and the classified known divergences - the engine's tests'
/// and the Lexbor reference's.
#[cfg(any(test, feature = "lexbor-compat"))]
pub mod conformance;
