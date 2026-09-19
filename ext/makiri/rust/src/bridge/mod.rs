//! The raw Ruby boundary.
//!
//! The ONLY layer allowed raw Ruby String access, verified-string minting,
//! TypedData, and the C calls that raise; everything else receives an
//! already-checked view. See docs/string_types.md for the type lattice these
//! functions move values through, and CLAUDE.md's "Text-input contract" for
//! the rules they enforce.
//!
//! Mostly primitives the unsafe-free `glue` builds its methods on. The classes
//! whose Ruby objects ARE a raw structure - `NodeSet`, `XPathContext`, the CSS
//! and serializer entries that fill a NodeSet or a buffer in place - define
//! their methods here, beside that structure. A failure comes back as
//! `Err(magnus::Error)` rather than as a raise: `ruby` holds the few places a
//! raising C function is still called, and turns each raise into an `Err`
//! there.

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

/// `Node#css` / `#at_css` / `#matches?` over the CSS selector engine.
#[cfg(feature = "lexbor")]
pub mod selectors;

/// The Ruby-facing fragment entry points (`Document#fragment`, `Node#parse`).
#[cfg(feature = "lexbor")]
pub mod fragment;

/// `Node#to_html` / `#inner_html` and the HTML serializer binding.
#[cfg(feature = "lexbor")]
pub mod serialize;

/// `Makiri::NodeSet`'s wrapper type (opaque node pointers + a Document
/// keepalive) and the safe fill handle over it.
#[cfg(feature = "lexbor")]
pub mod node_set;

/// The Ruby <-> XPath engine seam: which backend a query runs on, and
/// building the engine context for a Ruby node or document.
#[cfg(feature = "lexbor")]
pub mod xpath;

pub mod ruby;

pub mod string;

/// The TypedData objects and their GC callbacks.
pub mod typed;

pub mod xml_decode;
