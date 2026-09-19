//! Lexbor's typed handles and compatibility facilities.
//!
//! `html` is the one reader of Lexbor's DOM structs: everything else here,
//! the index builders included, walks the tree through its typed handles.
//! (What else reads Lexbor memory reads something that is not the DOM:
//! `arena_bytes` the arena's pool chunks, `source_loc` the tokenizer's tokens.)
//! Around it, everything Lexbor does not give us and we will not patch it to:
//! the attribute->owner index, source locations, the text index, cross-import,
//! and the input sanitiser.

pub mod arena_bytes;
pub mod cross_import;
pub mod dom_index;
pub mod html;
pub mod post_parse;
pub mod source_loc;
pub mod text_index;
pub mod utf8_input;
