//! The Ruby boundary's text layer, ported from ext/makiri/bridge/.
//!
//! This is the ONLY layer allowed raw Ruby String access and verified-string
//! minting; everything else receives an already-checked view. See
//! docs/string_types.md for the type lattice these functions move values
//! through, and CLAUDE.md's "Text-input contract" for the rules they enforce.
//!
//! Nothing here defines a Ruby method - these are functions the rest of the
//! extension calls. A failure comes back as `Err(magnus::Error)` rather than
//! as a raise: `ruby` holds the few places a raising C function is still
//! called, and turns each raise into an `Err` there.

pub mod alloc;

pub mod gvl;

/// The Ruby <-> Lexbor DOM seam.
#[cfg(feature = "lexbor")]
pub mod lexbor;

/// The Ruby <-> XML-arena DOM seam (the counterpart of `lexbor`).
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
