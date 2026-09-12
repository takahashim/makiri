//! The Lexbor gap-fillers (ext/makiri/dom_adapter/).
//!
//! Everything Lexbor does not give us and we will not patch it to: the
//! attribute->owner index, source locations, the text index, cross-import, and
//! the input sanitiser. Each file moves behind its own `MAKIRI_RUST_DOM_*` flag.

#[cfg(feature = "dom-text-index")]
pub mod text_index;
#[cfg(feature = "dom-utf8-input")]
pub mod utf8_input;
