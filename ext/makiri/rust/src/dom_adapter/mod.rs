//! The Lexbor gap-fillers (ext/makiri/dom_adapter/).
//!
//! Everything Lexbor does not give us and we will not patch it to: the
//! attribute->owner index, source locations, the text index, cross-import, and
//! the input sanitiser. Each moved behind its own flag while the C half still
//! existed; the features remain as the compilation units, one per former file.

#[cfg(feature = "dom-cross-import")]
pub mod cross_import;
#[cfg(feature = "dom-index")]
pub mod dom_index;
#[cfg(feature = "dom-post-parse")]
pub mod post_parse;
#[cfg(feature = "dom-source-loc")]
pub mod source_loc;
#[cfg(feature = "dom-text-index")]
pub mod text_index;
#[cfg(feature = "dom-utf8-input")]
pub mod utf8_input;
