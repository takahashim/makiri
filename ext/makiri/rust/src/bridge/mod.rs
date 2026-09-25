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
//! A failure comes back as `Err(magnus::Error)` rather than as a raise: `ruby`
//! holds the few places a raising C function is still called, and turns each
//! raise into an `Err` there.

pub mod alloc;

pub mod gvl;

/// The Ruby <-> Lexbor DOM seam.
/// The node and Document wrappers, their TypedData types, and the
/// representation-agnostic accessors.
#[cfg(feature = "lexbor")]
pub mod wrapper;

/// The HTML front door: Lexbor nodes to Ruby and back, and the HTML edits.
#[cfg(feature = "lexbor")]
pub mod html;

/// The XML front door and the one gated way to write an arena (the XML
/// counterpart of `html`).
#[cfg(feature = "lexbor")]
pub mod xml;

/// The Ruby <-> Document seam: parsing (GVL-released), the Document readers, the
/// fragment pipeline, and cross-document import/clone.
#[cfg(feature = "lexbor")]
pub mod doc;

/// The Ruby-facing fragment entry points (`Document#fragment`, `Node#parse`).
#[cfg(feature = "lexbor")]
pub mod fragment;

/// `Makiri::NodeSet`'s wrapper type (opaque node pointers + a Document
/// keepalive) and the safe fill handle over it.
#[cfg(feature = "lexbor")]
pub mod node_set;

/// Reading a stored node word back as a typed wrapper (the two front doors).
#[cfg(feature = "lexbor")]
pub mod node_wrap;

/// The Ruby <-> XPath engine seam: which backend a query runs on, and
/// building the engine context for a Ruby node or document.
#[cfg(feature = "lexbor")]
pub mod xpath;

pub mod ruby;

pub mod string;

/// The TypedData objects and their GC callbacks.
pub mod typed;

pub mod xml_decode;
