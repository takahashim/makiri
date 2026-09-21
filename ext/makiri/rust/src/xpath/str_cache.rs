//! The per-evaluation string-value cache.

#![forbid(unsafe_code)]

use super::abi::*;
use crate::err_setf;
use crate::falloc::Reserve;
use crate::ptr_table::PtrMap;
use crate::token::Token;

/// Where a string-value sits in the cache that returned it.
///
/// An index rather than a borrow, so a caller can hold one string-value while
/// asking the cache for the next - the node-set comparisons hold one side's
/// value across the whole scan of the other.
#[derive(Clone, Copy, Debug)]
pub struct TextId(usize);

/// One evaluate's node string-value cache: an ordered store of the texts it
/// built, plus a token-keyed index into it.
///
/// It owns every text it holds, and they go with it - there is nothing to
/// clear by hand.
#[derive(Default)]
pub struct StrCache {
    entries: Vec<(Token, Text)>,
    /// node token -> entry index.
    index: PtrMap<Token, usize>,
    total_bytes: usize,
}

impl StrCache {
    pub const fn new() -> StrCache {
        StrCache {
            entries: Vec::new(),
            index: PtrMap::new(),
            total_bytes: 0,
        }
    }

    /// The cached text of `node`, if one is.
    #[inline]
    pub fn find(&self, node: Token) -> Option<TextId> {
        self.index.get(node).map(TextId)
    }

    /// The text `id` names.
    #[inline]
    pub fn text(&self, id: TextId) -> &[u8] {
        self.entries[id.0].1.as_slice()
    }

    /// Cache `text` as `node`'s string-value, within `budget`'s string cap on
    /// the total cached bytes.
    ///
    /// Every refusal happens before anything is committed, so a failed insert
    /// leaves the cache as it was (and drops `text`).
    pub fn insert(
        &mut self,
        node: Token,
        text: Text,
        budget: &mut Budget,
    ) -> Result<TextId, Reported> {
        if self.entries.falloc_reserve(1).is_err() {
            return Err(err_setf!(
                budget.sink(),
                XP_ERR_OOM,
                "out of memory in node string cache"
            ));
        }

        /* A total cap on the cached bytes, so one evaluate cannot grow the cache
         * without bound. */
        let Some(new_total) = self.total_bytes.checked_add(text.as_slice().len()) else {
            return Err(err_setf!(
                budget.sink(),
                XP_ERR_OOM,
                "node string cache size overflow"
            ));
        };
        budget.check_string_bytes(new_total)?;

        /* Index before committing: a failed growth leaves the map as it was,
         * and the entry push below cannot fail - it was reserved above. */
        let id = self.entries.len();
        if self.index.insert(node, id).is_err() {
            return Err(err_setf!(
                budget.sink(),
                XP_ERR_OOM,
                "out of memory indexing node string cache"
            ));
        }
        self.total_bytes = new_total;
        self.entries.push((node, text));
        Ok(TextId(id))
    }
}
