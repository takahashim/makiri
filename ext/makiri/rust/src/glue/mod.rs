//! The Ruby boundary, ported from ext/makiri/glue/.
//!
//! Unlike the XPath port, this layer does not replace a C ABI with an identical
//! one - it replaces C that calls Ruby with Rust that calls Ruby through
//! [magnus]. What it *does* preserve is the seam: `Init_makiri` still creates
//! every class and module and still calls one `mkr_init_<feature>()` per
//! feature, so a feature moved language without anything else moving. That seam
//! is why the port could land one file at a time: each `MAKIRI_RUST_GLUE_*` flag
//! swapped one `mkr_init_*` implementation and dropped the C file that defined
//! it. The flags are gone with the C; the seam stayed, and `init.rs` still calls
//! the same twelve entry points.
//!
//! # Two rules this layer lives by
//!
//! **Never let C longjmp through a Rust frame.** `rb_raise` unwinds with
//! `longjmp`, which skips Rust destructors: a raise crossing a frame that owns a
//! `Vec`, an [`mkr_buf_t`](crate::cbuf::Buf) or any other resource leaks it. So a
//! raising C accessor - `mkr_html_node_unwrap` and its kind - is called only
//! where nothing needs dropping, which in practice means first, before any
//! buffer exists. Everything Rust itself reports travels back as
//! `Result<_, magnus::Error>`, which magnus turns into a raise after the Rust
//! frames have returned normally.
//!
//! **Nothing Ruby crosses into a GVL-released closure.** Not a `Value`, not a
//! `Ruby` handle. The C glue already works this way (parse copies its input to a
//! C buffer before releasing), and the constraint is the same here.

pub mod abi;

#[cfg(feature = "glue-css")]
pub mod css;
/// Makiri::HTML::Document.
#[cfg(feature = "glue-doc")]
pub mod doc;
/// The HTML fragment pipeline, which doc.rs and two C files both use.
#[cfg(feature = "glue-doc")]
pub mod fragment;
/// Makiri::Lexbor::CSS.parse_stylesheet - the thin stylesheet binding.
#[cfg(feature = "glue-lexbor-css")]
pub mod lexbor_css;

#[cfg(feature = "glue-node")]
pub mod node;

#[cfg(feature = "glue-node-set")]
pub mod node_set;

#[cfg(feature = "glue-serialize")]
pub mod serialize;

#[cfg(feature = "glue-xml")]
pub mod xml;

#[cfg(feature = "glue-xpath")]
pub mod xpath;

#[cfg(feature = "glue-xml-node-read")]
pub mod xml_node;

#[cfg(feature = "glue-html-node")]
pub mod html_node;
