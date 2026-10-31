//! The parser's static resource tables, as Rust data. Two are GENERATED, not
//! written, each from the source that defines it, and one is kept:
//!
//! - [`tree`] - the tree builder's categories and adjustment tables - from
//!   the HTML Standard's `source` at the pinned commit, and [`entities`] -
//!   the named character references, in Lexbor's search-tree layout - from
//!   the WHATWG `entities.json`. Their generator is
//!   `conformance::res_gen`, which needs no Lexbor:
//!
//!   ```text
//!   ruby spec/conformance/parser_data.rb     # fetch the pinned sources
//!   MAKIRI_REGENERATE=1 cargo test --no-default-features res_gen -- --include-ignored
//!   ```
//!
//! - [`names`] - tags, attributes, namespaces and prefixes, with their ids -
//!   is the crate's own table, not regenerated: the Standard numbers
//!   nothing, so the ids are the ones Lexbor's tables gave, kept as they are.
//!   Adding a name is a decision of its own.
//!
//! While the reference Lexbor is linked, `lexbor::res_tests` holds all three
//! to Lexbor's tables, entry by entry (`docs/parser_differences.ja.md`, T1,
//! is the one entry `tree` departs from on purpose).
//!
//! The ids keep Lexbor's numbering because the rest of the crate reads them:
//! the CSS matcher and the element index index arrays by tag id, and the tree
//! builder switches on them.

#[rustfmt::skip]
pub mod entities;
#[rustfmt::skip]
pub mod names;
#[rustfmt::skip]
pub mod tree;

/// One node of a static search tree over bytes (Lexbor's
/// `lexbor_sbst_entry_static_t`): within one layer the nodes form a binary
/// search tree on `key` through `left` and `right`, and `next` is the root of
/// the layer for the following byte. Index 0 is the "none" node.
///
/// The replacement is an index into a separate table rather than a slice in
/// the entry, so the tree the tokenizer walks is ten bytes a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SbstEntry {
    pub key: u8,
    /// 1 + the index in [`entities::ENTITY_VALUES`] of the replacement for the
    /// name that ends here; 0 when none does.
    pub value: u16,
    pub left: u16,
    pub right: u16,
    pub next: u16,
}

impl SbstEntry {
    /// The replacement for the name that ends here, if one does.
    #[inline]
    pub fn value(&self) -> Option<&'static [u8]> {
        let i = (self.value as usize).checked_sub(1)?;
        entities::ENTITY_VALUES.get(i).copied()
    }
}

/// The entry for `key` in the layer rooted at `root`, with its index, or
/// `None` (`lexbor_sbst_entry_static_find`).
#[inline]
pub fn sbst_find(
    table: &'static [SbstEntry],
    mut root: usize,
    key: u8,
) -> Option<(usize, &'static SbstEntry)> {
    while root != 0 {
        let e = table.get(root)?;
        if e.key == key {
            return Some((root, e));
        }
        root = if key > e.key { e.right } else { e.left } as usize;
    }
    None
}
