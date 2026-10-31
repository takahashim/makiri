//! makiri - the Ruby extension behind the `makiri` gem: the Ruby surface
//! (`bridge`, `glue`, `init`) over the engine (`makiri_engine`), on the binding
//! layer (`nupur`).
//!
//! The engine is a crate of its own, which knows neither Ruby nor Lexbor; this
//! crate re-exports its modules at its root, so `crate::xml` here is the
//! engine's. Lexbor is not in this crate's dependency graph at all: it is the
//! reference the ports are tested against (`makiri_lexbor`).
//!
//! No SYMBOL here carries the C ABI's `mkr_` prefix any more: the crate exports only
//! `Init_makiri` (and `ruby_abi_version`), so every item is named as Rust.

#![deny(unsafe_code)]
/* An `Err` of a Ruby error is never dropped: `Error::rescue` handles the
 * classes it names and hands every exit back. `forbid`, so no `allow`
 * reopens the rule. */
#![cfg_attr(dylint_lib = "makiri_lints", forbid(discarded_ruby_error))]
// A panic unwinds into `Makiri::InternalError` at a method's trampoline (or
// `fatal` outside one) - a broken invariant, never an answer. `Cargo.toml`'s clippy lints therefore refuse a
// new `unwrap()` or `panic!` in the shipped crate; a site that genuinely wants
// one carries an `#[allow]` saying why. Test code is exempt: there a panic IS
// the failure report.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

/// The engine's modules, by the paths the Ruby layer names them with.
pub use makiri_engine::{
    ceiling_buf, child_index, compat, css_cache, css_match, doc_cell, dom_rules, engine_error,
    html, html_dom, kani_bounds, limits, no_unwind, node_type, ptr_table, selectors, source_lines,
    stack, text, token, utf8_decode, utf8_input, xml, xpath,
};

/// The Ruby binding layer the bridge is built on, a crate of its own that
/// knows nothing of Makiri (`nupur/`). Named `crate::nupur` here, and its
/// `method!` / `function!` macros are in scope crate-wide.
extern crate nupur;

/// The method-registration macros, by the path the glue imports them from.
pub(crate) use nupur::{function, method};

/// `nupur::msg`, under the name the bridge and the glue use.
pub use nupur::msg;

/// The Ruby boundary - the only part of the crate that depends on rb-sys.
pub mod bridge;
pub mod glue;

/// `rake dylint`'s self-test: what the crate's own lints must report.
#[cfg(makiri_lint_probe)]
mod lint_probe;

/// `Init_makiri` and the class hierarchy: the symbol Ruby looks up at require
/// time, and the one export the extension publishes.
pub mod init;
