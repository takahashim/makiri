//! The raw Ruby boundary.
//!
//! The ONLY layer allowed raw Ruby String access, verified-string minting,
//! TypedData, and the C calls that raise; everything else receives an
//! already-checked view. See docs/string_types.md for the type lattice these
//! functions move values through, and CLAUDE.md's "Text-input contract" for
//! the rules they enforce.
//!
//! Primitives the unsafe-free `glue` builds every Ruby method on: this layer
//! defines no method and reads no argument list. Where a Ruby object IS a raw
//! structure - `NodeSet`, `XPathContext` - the structure lives here with the
//! operations that must keep its invariants (which nodes may enter a set, what
//! a running evaluate refuses), and the methods over them are the glue's.
//! `rake unsafe:boundaries` holds that: no method is defined, and no argument
//! list scanned, in this layer.
//!
//! A failure comes back as `Err(crate::bridge::rb::Error)` rather than as a
//! raise: every raising C call is `nupur::raw`'s, which turns its raise into
//! an `Err` there.
//!
//! Unsafe-free, from this root down: what reads Ruby's memory or calls its C
//! is `nupur`'s, behind safe types, and this layer is Makiri's rules over
//! them.

#![forbid(unsafe_code)]

/// The Arrays, Hashes, numbers and Symbols the glue builds, allocation
/// failures as `Err`.
pub use crate::nupur::build;

/// The Ruby C calls that can raise, each with its raise stated.
pub use crate::nupur::raw;

/// The Ruby layer over rb-sys: values, errors, method registration, classes.
pub use crate::nupur::rb;

pub use crate::nupur::gvl;

/// `ruby_stack_check`, installed into `crate::stack` at `Init_makiri` so the
/// engine's recursion checks can consult it without depending on Ruby.
pub mod stack;

/// The Ruby pending-interrupt check (`nupur::interrupt::interrupt_probe`), installed
/// into `crate::interrupt` at `Init_makiri` so the engine's budgets can ask it.
pub mod interrupt;

/// The node and Document wrappers, their TypedData types, and the
/// representation-agnostic accessors.
#[cfg(feature = "html-dom")]
pub mod wrapper;

/// The bridge steps that differ between the two HTML DOMs.
pub mod dom;

/// The HTML front door: arena nodes to Ruby and back, and the HTML edits.
#[cfg(feature = "html-dom")]
pub mod html;

/// The XML front door and the one gated way to write an arena (the XML
/// counterpart of `html`).
#[cfg(feature = "html-dom")]
pub mod xml;

/// The Ruby <-> Document seam: parsing (GVL-released), the Document readers, the
/// fragment pipeline, and cross-document import/clone.
#[cfg(feature = "html-dom")]
pub mod doc;

/// The Ruby-facing fragment entry points (`Document#fragment`, `Node#parse`).
#[cfg(feature = "html-dom")]
pub mod fragment;

/// `Makiri::NodeSet`'s wrapper type (opaque node pointers + a Document
/// keepalive) and the safe fill handle over it.
#[cfg(feature = "html-dom")]
pub mod node_set;

/// Reading a stored node word back as a typed wrapper (the two front doors).
#[cfg(feature = "html-dom")]
pub mod node_wrap;

/// The errors the DOM's insertion rules raise, for HTML and XML alike.
pub mod dom_error;

/// The Ruby <-> XPath engine seam: which backend a query runs on, and
/// building the engine context for a Ruby node or document.
#[cfg(feature = "html-dom")]
pub mod xpath;

pub mod ruby;

pub mod string;

/// The TypedData objects and their GC callbacks.
pub use crate::nupur::typed;

pub mod xml_decode;
