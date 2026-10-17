//! The names a document interns: tag names and attribute local names.
//!
//! Lexbor keeps two hash tables per document (`tags` and `attrs`) that the
//! tokenizer and the DOM share: a name in Lexbor's static table resolves to
//! its static id, and any other name is appended, lower-cased, and gets an id
//! of its own (`lxb_tag_append_lower`, `lxb_dom_attr_local_name_append`).
//! Lexbor's appended ids are pointer values; here they are numbers above the
//! static range, which is all any reader asks of them (equality, and "is it
//! static", which is `id < *__LAST_ENTRY`).

use crate::falloc::{try_to_vec, MapInsert, VecPush};
use crate::html::res::{attr_id_by_lower_name, names as res, tag_id_by_lower_name};
use std::collections::HashMap;

/// An allocation the table needed failed; nothing was added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamesOom;

/// One table: names appended past the static ones.
#[derive(Default)]
struct Appended {
    ids: HashMap<Vec<u8>, usize>,
    names: Vec<Vec<u8>>,
}

impl Appended {
    /// The id for `lower` (already lower-cased), appending it at
    /// `first + index` when new.
    fn append(&mut self, lower: &[u8], first: usize) -> Result<usize, NamesOom> {
        if let Some(&id) = self.ids.get(lower) {
            return Ok(id);
        }
        let id = first + self.names.len();
        let key = try_to_vec(lower).ok_or(NamesOom)?;
        let name = try_to_vec(lower).ok_or(NamesOom)?;
        self.names.falloc_push(name).map_err(|()| NamesOom)?;
        if self.ids.falloc_insert(key, id).is_err() {
            self.names.pop();
            return Err(NamesOom);
        }
        Ok(id)
    }

    fn name(&self, id: usize, first: usize) -> Option<&[u8]> {
        self.names.get(id.checked_sub(first)?).map(Vec::as_slice)
    }
}

/// A document's interned tag and attribute names.
#[derive(Default)]
pub struct Names {
    tags: Appended,
    attrs: Appended,
    /// Scratch for lower-casing a name before it is looked up.
    lower: Vec<u8>,
}

/// The first id an appended tag gets. Lexbor reserves `LAST_ENTRY` itself
/// (its by-id lookup answers "none" for it), so appended ids start past it.
pub const FIRST_APPENDED_TAG: usize = res::TAG__LAST_ENTRY + 1;
/// As [`FIRST_APPENDED_TAG`], for attribute names.
pub const FIRST_APPENDED_ATTR: usize = res::ATTR__LAST_ENTRY + 1;

impl Names {
    fn lowered(&mut self, name: &[u8]) -> Result<(), NamesOom> {
        self.lower.clear();
        self.lower.falloc_reserve_exact_bytes(name.len())?;
        self.lower.extend(name.iter().map(u8::to_ascii_lowercase));
        Ok(())
    }

    /// `lxb_tag_append_lower`: the tag id for `name`, ASCII case-insensitively,
    /// appending it (lower-cased) when Lexbor's static table lacks it.
    pub fn tag_append_lower(&mut self, name: &[u8]) -> Result<usize, NamesOom> {
        self.lowered(name)?;
        if let Some(id) = tag_id_by_lower_name(&self.lower) {
            return Ok(id);
        }
        let Names { tags, lower, .. } = self;
        tags.append(lower, FIRST_APPENDED_TAG)
    }

    /// `lxb_dom_attr_local_name_append`: as [`tag_append_lower`](Self::tag_append_lower),
    /// for an attribute's local name.
    pub fn attr_local_name_append(&mut self, name: &[u8]) -> Result<usize, NamesOom> {
        self.lowered(name)?;
        if let Some(id) = attr_id_by_lower_name(&self.lower) {
            return Ok(id);
        }
        let Names { attrs, lower, .. } = self;
        attrs.append(lower, FIRST_APPENDED_ATTR)
    }

    /// The tag id for `name` if it is already known (`lxb_tag_id_by_name`),
    /// without appending.
    pub fn tag_id(&mut self, name: &[u8]) -> Result<Option<usize>, NamesOom> {
        self.lowered(name)?;
        Ok(tag_id_by_lower_name(&self.lower).or_else(|| self.tags.ids.get(&self.lower).copied()))
    }

    /// A tag id's lower-case name.
    pub fn tag_name(&self, id: usize) -> Option<&[u8]> {
        match res::TAG_NAMES.get(id) {
            Some(n) => Some(n),
            None => self.tags.name(id, FIRST_APPENDED_TAG),
        }
    }

    /// An attribute id's lower-case local name.
    pub fn attr_name(&self, id: usize) -> Option<&[u8]> {
        match res::ATTR_NAMES.get(id) {
            Some(n) => Some(n),
            None => self.attrs.name(id, FIRST_APPENDED_ATTR),
        }
    }
}

/// Reserving bytes for the lower-cased copy.
trait ReserveBytes {
    fn falloc_reserve_exact_bytes(&mut self, n: usize) -> Result<(), NamesOom>;
}

impl ReserveBytes for Vec<u8> {
    fn falloc_reserve_exact_bytes(&mut self, n: usize) -> Result<(), NamesOom> {
        use crate::falloc::Reserve;
        self.falloc_reserve(n).map_err(|()| NamesOom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_names_keep_lexbor_ids_and_others_are_appended_once() {
        let mut n = Names::default();
        assert_eq!(n.tag_append_lower(b"DiV"), Ok(res::TAG_DIV));
        let x = n.tag_append_lower(b"my-Element").expect("appends");
        assert!(x >= FIRST_APPENDED_TAG);
        assert_eq!(n.tag_append_lower(b"MY-ELEMENT"), Ok(x));
        assert_eq!(n.tag_name(x), Some(&b"my-element"[..]));
        assert_eq!(n.tag_id(b"my-element"), Ok(Some(x)));
        assert_eq!(n.tag_id(b"other"), Ok(None));
        assert_eq!(n.attr_local_name_append(b"CLASS"), Ok(res::ATTR_CLASS));
        let a = n.attr_local_name_append(b"data-x").expect("appends");
        assert_eq!(n.attr_name(a), Some(&b"data-x"[..]));
        assert_ne!(a, x - FIRST_APPENDED_TAG + FIRST_APPENDED_ATTR + 1);
    }
}
