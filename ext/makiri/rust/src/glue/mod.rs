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
//! `Vec`, an [`mkr_buf_t`](crate::cbuf::Buf), a `RefCell` borrow or any other
//! resource leaks it or leaves it held. So a failure travels back as
//! `Result<_, magnus::Error>`, which magnus turns into a raise after the Rust
//! frames have returned normally. The node unwraps and the text checks return
//! `Err` for exactly that reason, and a Ruby C function that can raise is called
//! through `bridge::ruby`, which catches the raise and hands it back the same
//! way. What still raises directly is `node_set_push`, an entry point called
//! with the C convention that has no `Result` to return. It longjmps, so its
//! callers must not own anything that needs dropping.
//!
//! **Nothing Ruby crosses into a GVL-released closure.** Not a `Value`, not a
//! `Ruby` handle. The C glue already works this way (parse copies its input to a
//! C buffer before releasing), and the constraint is the same here.

pub mod abi;

pub mod css;
/// Makiri::HTML::Document.
pub mod doc;
/// The HTML fragment pipeline, which doc.rs and two C files both use.
pub mod fragment;
/// Makiri::Lexbor::CSS.parse_stylesheet - the thin stylesheet binding.
pub mod lexbor_css;

pub mod node;

pub mod node_set;

pub mod serialize;

pub mod xml;

pub mod xpath;

pub mod xml_node;

pub mod html_node;
