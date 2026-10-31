//! makiri_lexbor - the vendored Lexbor, as the reference the engine's ports
//! are held to: Lexbor's tree, selector and stylesheet parsers and CSS engine,
//! read through generated bindings, beside what the engine answers.
//!
//! Never part of a build that ships: the extension does not depend on this,
//! and this crate is a workspace of its own, so not even the extension's
//! Cargo.lock holds anything of Lexbor's. It depends on the engine (with
//! `lexbor-compat`, the switch that runs the ports as Lexbor did), whose
//! modules it re-exports at its root, so `crate::html` here is the engine's.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

/// The engine's modules, by the paths the reference names them with.
pub use makiri_engine::{
    ceiling_buf, child_index, compat, conformance, css_cache, css_match, doc_cell, dom_rules,
    engine_error, html, html_dom, kani_bounds, limits, no_unwind, node_type, ptr_table, selectors,
    source_lines, stack, text, token, utf8_decode, utf8_input, xml, xpath,
};

/// Catching a panic at a C boundary and re-raising it past one, so a panic in a
/// Lexbor callback becomes a panic of its caller rather than an abort.
pub mod caught;

/// Holding the GVL as a value: the token the process-global Lexbor CSS engines
/// ask for, and the cell that keeps them. Outside Ruby it is a lock.
pub mod gvl;

/// The vendored Lexbor boundary. This owns every direct use of its C ABI;
/// `adapter` supplies the compatibility facilities Lexbor does not provide.
pub mod lexbor;
