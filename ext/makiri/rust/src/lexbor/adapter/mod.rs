//! Lexbor's typed handles and compatibility facilities.
//!
//! `html` is the one reader of Lexbor's DOM structs: everything else here,
//! the index builders included, walks the tree through its typed handles.
//! (What else reads Lexbor memory reads something that is not the DOM:
//! `arena_bytes` the arena's pool chunks, `source_loc` the tokenizer's tokens,
//! `tree_guard` the tree builder's stack of open elements.)
//! Around it, everything Lexbor does not give us and we will not patch it to:
//! the element (tag-bucket) index, source locations, the text index, the
//! tree-depth guard, and cross-import.

pub mod arena_bytes;
pub mod cross_import;
pub mod dom_index;
pub mod html;
pub mod post_parse;
pub mod source_loc;
pub mod text_index;
pub mod tree_guard;

/// An allocation the adapter needed could not be satisfied.
///
/// Every step the adapter takes - storing a value or a name through Lexbor,
/// copying a node, growing a worklist, building an index - fails only for want
/// of memory, so one type carries all of them. Lexbor's own status is not
/// carried: no caller reports more than that the step failed. The caller fails
/// closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterOom;

impl crate::falloc::Oom for AdapterOom {
    #[inline]
    fn oom() -> Self {
        AdapterOom
    }
}

/// `cross_import` is the one place an [`AdapterOom`] meets the XML side; this
/// lets it propagate one with a bare `?` instead of a `.map_err(|_|
/// MutError::Oom)` at each crossing, the same shape as
/// `From<BudgetError> for MutError` in `xml::model`. It lives here, not
/// there: `xml` stays Lexbor-free, so the conversion has to sit on the side
/// that already depends on both.
impl From<AdapterOom> for crate::xml::model::MutError {
    #[inline]
    fn from(_: AdapterOom) -> Self {
        crate::xml::model::MutError::Oom
    }
}

/// As above, for the XPath engine's own error kind - the other place an
/// [`AdapterOom`] (rebuilding the element index for `//tag`) needs to become
/// something the rest of the engine understands.
impl From<AdapterOom> for crate::engine_error::ErrorKind {
    #[inline]
    fn from(_: AdapterOom) -> Self {
        crate::engine_error::ErrorKind::Oom
    }
}
