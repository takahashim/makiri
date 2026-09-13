//! Placeholder build script for the `links = "lexbor_static"` declaration in
//! Cargo.toml. The actual Lexbor static archive is linked by the parent crate's
//! build.rs; this script exists only to satisfy Cargo's requirement that a
//! package with a `links` key has a build script.
fn main() {}
