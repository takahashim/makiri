//! Placeholder build script for the cargo-fuzz crate.
//!
//! This file exists because rake-compiler's `source_pattern` matches `.rs`
//! files under `ext/makiri/rust` and stages them for the extension build.
//! The actual Lexbor static archive is linked by the parent crate's
//! `build.rs`; this script intentionally does nothing.
fn main() {}
