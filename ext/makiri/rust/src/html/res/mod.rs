//! The parser's static resource tables, as Rust data. All three are
//! GENERATED, not written, and each from the source that defines it:
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
//! - [`names`] - tags, attributes, namespaces and prefixes, with Lexbor's own
//!   id numbers - from Lexbor's static tables, by `lexbor::res_tests`. The
//!   Standard numbers nothing, so the ids stay Lexbor's.
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

/// The static tag id for an ASCII-lower-case `name`, if Lexbor has one -
/// the special ids (`#text`, `!--`, ...) included, as Lexbor's static lookup
/// holds them.
pub fn tag_id_by_lower_name(name: &[u8]) -> Option<usize> {
    names::TAG_BY_NAME
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| names::TAG_BY_NAME[i].1)
}

/// The static attribute id for an ASCII-lower-case local `name`, if Lexbor has
/// one.
pub fn attr_id_by_lower_name(name: &[u8]) -> Option<usize> {
    names::ATTR_BY_NAME
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| names::ATTR_BY_NAME[i].1)
}

/// The static namespace id for a namespace URI, matched byte for byte.
pub fn ns_id_by_link(link: &[u8]) -> Option<usize> {
    names::NS_LINKS
        .iter()
        .enumerate()
        .skip(names::NS_HTML)
        .find(|(_, l)| **l == link)
        .map(|(i, _)| i)
}
