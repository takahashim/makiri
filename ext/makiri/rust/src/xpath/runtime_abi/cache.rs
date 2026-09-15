//! The per-evaluation string-value cache, and the pointer hash every
//! pointer-keyed table shares.
use super::super::abi::*;
use super::super::value::Text;
use crate::err_setf;
use crate::falloc::{try_vec_with_capacity, Reserve};
use core::ffi::c_void;

/// The MurmurHash3 fmix64 finalizer over a pointer value.
///
/// One definition for every pointer-keyed table: the string-value cache, the
/// document-order index and the DOM indexes all hash the same way.
#[inline]
pub fn ptr_hash<T>(p: *const T) -> u64 {
    let mut h = p as usize as u64;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    h
}

/// Where a string-value sits in the cache that returned it.
///
/// An index rather than a borrow, so a caller can hold one string-value while
/// asking the cache for the next - the node-set comparisons hold one side's
/// value across the whole scan of the other.
#[derive(Clone, Copy, Debug)]
pub struct TextId(usize);

/// One evaluate's node string-value cache: an ordered store of the texts it
/// built, plus a pointer-keyed open-addressing index into it.
///
/// It owns every text it holds, and they go with it - there is nothing to
/// clear by hand.
pub struct StrCache {
    entries: Vec<(*const c_void, Text)>,
    /// node pointer -> entry index + 1; 0 is an empty slot. Empty, or a power
    /// of two at most half full.
    buckets: Vec<usize>,
    total_bytes: usize,
}

impl Default for StrCache {
    fn default() -> Self {
        StrCache::new()
    }
}

impl StrCache {
    pub const fn new() -> StrCache {
        StrCache {
            entries: Vec::new(),
            buckets: Vec::new(),
            total_bytes: 0,
        }
    }

    /// The cached text of `node`, if one is.
    #[inline]
    pub fn find(&self, node: *const c_void) -> Option<TextId> {
        if self.buckets.is_empty() {
            return None;
        }
        let mask = self.buckets.len() - 1;
        let mut j = (ptr_hash(node) as usize) & mask;
        loop {
            let slot = self.buckets[j];
            if slot == 0 {
                return None;
            }
            if core::ptr::eq(self.entries[slot - 1].0, node) {
                return Some(TextId(slot - 1));
            }
            j = (j + 1) & mask;
        }
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
        node: *const c_void,
        text: Text,
        budget: &mut Budget,
    ) -> Result<TextId, Reported> {
        if self.entries.mkr_reserve(1).is_err() {
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

        /* Grow the index before committing. It rebuilds only from the entries
         * already there, so the new entry goes in once nothing can fail. Load
         * factor stays at or below 1/2. */
        if self.buckets.is_empty() || (self.entries.len() + 1) * 2 > self.buckets.len() {
            let new_cap = if self.buckets.is_empty() {
                64
            } else {
                let Some(cap) = self.buckets.len().checked_mul(2) else {
                    return Err(err_setf!(
                        budget.sink(),
                        XP_ERR_OOM,
                        "node string cache index overflow"
                    ));
                };
                cap
            };
            if self.reindex(new_cap).is_err() {
                return Err(err_setf!(
                    budget.sink(),
                    XP_ERR_OOM,
                    "out of memory indexing node string cache"
                ));
            }
        }

        let id = self.entries.len();
        self.total_bytes = new_total;
        self.entries.push((node, text));
        self.index_put(id);
        Ok(TextId(id))
    }

    /// Replace the index with one of `cap` slots over the current entries.
    fn reindex(&mut self, cap: usize) -> Result<(), ()> {
        let mut buckets = try_vec_with_capacity(cap).ok_or(())?;
        buckets.resize(cap, 0);
        self.buckets = buckets;
        for i in 0..self.entries.len() {
            self.index_put(i);
        }
        Ok(())
    }

    /// Point a free slot at entry `i`. The index has room: it is at most half
    /// full before this.
    fn index_put(&mut self, i: usize) {
        let mask = self.buckets.len() - 1;
        let mut j = (ptr_hash(self.entries[i].0) as usize) & mask;
        while self.buckets[j] != 0 {
            j = (j + 1) & mask;
        }
        self.buckets[j] = i + 1;
    }
}
