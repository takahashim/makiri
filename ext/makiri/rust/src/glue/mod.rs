//! The Ruby boundary, ported from ext/makiri/glue/.
//!
//! Unlike the XPath port, this layer does not replace a C ABI with an identical
//! one - it replaces C that calls Ruby with Rust that calls Ruby through
//! [magnus]. What it *does* preserve is the seam: `Init_makiri` still creates
//! every class and module and still calls one `mkr_init_<feature>()` per
//! feature, so a feature moves language without anything else moving. Each
//! `MAKIRI_RUST_GLUE_*` flag swaps one of those `mkr_init_*` implementations and
//! drops the C file that defined it, exactly as the engine flags do.
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

#[cfg(feature = "glue-node")]
pub mod node;

#[cfg(feature = "glue-node-set")]
pub mod node_set;

#[cfg(feature = "glue-serialize")]
pub mod serialize;

#[cfg(feature = "glue-xml")]
pub mod xml;

#[cfg(feature = "glue-xml-node-read")]
pub mod xml_node;
