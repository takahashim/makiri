//! Lexbor's typed handles and compatibility facilities.
//!
//! `html` is the one reader of Lexbor's DOM structs: everything else here,
//! the index builders included, walks the tree through its typed handles.
//! (What else reads Lexbor memory reads something that is not the DOM:
//! `arena_bytes` the arena's pool chunks, `source_loc` the tokenizer's tokens.)
//! Around it, everything Lexbor does not give us and we will not patch it to:
//! the attribute->owner index, source locations, the text index, and
//! cross-import.

pub mod arena_bytes;
pub mod cross_import;
pub mod dom_index;
pub mod html;
pub mod post_parse;
pub mod source_loc;
pub mod text_index;

/// An allocation the adapter needed could not be satisfied.
///
/// Every step the adapter takes - storing a value or a name through Lexbor,
/// copying a node, growing a worklist, building an index - fails only for want
/// of memory, so one type carries all of them. Lexbor's own status is not
/// carried: no caller reports more than that the step failed. The caller fails
/// closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterOom;
