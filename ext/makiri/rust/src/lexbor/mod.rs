//! The sole owner of the vendored Lexbor FFI boundary.
//!
//! Raw `lxb_*` bindings, generated layouts, and C callbacks stay below this
//! module. Higher layers use the typed handles exported by `adapter`.

pub mod adapter;
/// The Lexbor CSS stylesheet parser and its raw callback traversal.
#[cfg(feature = "ruby")]
pub mod stylesheet;
