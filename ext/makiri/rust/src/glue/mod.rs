//! The Ruby boundary, ported from ext/makiri/glue/.
//!
//! Unlike the XPath port, this layer does not replace a C ABI with an identical
//! one - it replaces C that calls Ruby with Rust that calls Ruby through
//! [magnus]. The port kept a per-feature `mkr_init_*` seam so a feature could
//! move a file at a time, and for a while the moved features stayed reachable
//! here as one-line `pub use` re-exports. Those are gone: a feature whose
//! implementation lives in `lexbor` (the selector engine, the stylesheet
//! binding, the serializer, the fragment pipeline) is registered from its
//! `lexbor` entry point, and what remains in `glue` is the Ruby API that has
//! not moved below the boundary yet.
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
//! way. `rake unsafe:boundaries` pins what is left, and it is worth being exact
//! about what it measures: it counts `rb_raise`, `rb_exc_raise`, `rb_jump_tag`
//! and `rb_check_typeddata` in every file OUTSIDE `bridge/`, and holds that
//! count at zero. So the claim is about this layer, not about the process.
//!
//! Inside the bridge those calls stay, by design. `bridge::ruby` makes four
//! `rb_check_typeddata` calls: one under `protect`, two that cannot raise
//! because the type was established first, and `typed_data_unprotected`, which
//! CAN raise and documents that no Rust destructor may be live when it does -
//! the per-node path, where magnus's protected conversion measured about a
//! quarter of the throughput of the C it replaced. What can still unwind is Ruby
//! itself - its allocator's `NoMemoryError`, or Ruby code a C call reaches -
//! which is why the loops that push into a NodeSet while owning a collection
//! run under `protect`.
//!
//! **Nothing Ruby crosses into a GVL-released closure.** Not a `Value`, not a
//! `Ruby` handle. The C glue already works this way (parse copies its input to a
//! C buffer before releasing), and the constraint is the same here.

pub mod abi;

/// Makiri::HTML::Document.
pub mod doc;

pub mod node;

pub mod node_set;

pub mod xml;

pub mod xpath;

pub mod xml_node;

pub mod html_node;
