//! Ruby registration seam for the Lexbor stylesheet facade.
//!
//! The parser, generated rule layouts, and callbacks live in
//! [`crate::lexbor::stylesheet`].  Keeping this re-export preserves the
//! `Init_makiri` call site while the Ruby conversion is moved behind bridge
//! helpers in a subsequent boundary-tightening step.

pub use crate::lexbor::stylesheet::init_lexbor_css;
