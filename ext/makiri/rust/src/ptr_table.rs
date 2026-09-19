//! Pointer-keyed tables: the one pointer hash, and the fixed-capacity,
//! insert-only open-addressing table the per-document indexes are built on.
//!
//! Every pointer-keyed structure in the crate - the XPath string-value cache,
//! the document-order index, the DOM and text indexes, the NodeSet set
//! operations - hashes with [`ptr_hash`]. It lives here, below all of them,
//! rather than in any one of their modules, so an index does not reach into
//! another layer's internals for a hash function.
//!
//! [`PtrTable`] is the shape two of them had written out separately: sized once
//! for a known number of keys, filled, then only read. Sizing once is what
//! makes it fail-closed - a build either gets its whole table or none - and a
//! load factor of at most 1/2 is what makes every probe terminate.

#![forbid(unsafe_code)]

use crate::falloc::try_vec_with_capacity;

/// The MurmurHash3 fmix64 finalizer over a pointer value.
///
/// Heap addresses are not attacker-chosen the way strings are, so a
/// SipHash-strength hash buys nothing here and costs a lot on the hot paths.
#[inline]
pub fn ptr_hash<T>(p: *const T) -> u64 {
    mix64(p as usize as u64)
}

/// The finalizer itself, for a caller that already holds the address as an
/// integer (a `Hasher` fed through `write_usize`).
#[inline]
pub fn mix64(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// A pointer -> `V` table for a key count known before it is filled.
///
/// Insert-only (there are no tombstones), sized at load factor <= 1/2 for the
/// count it was built for, and never grown: inserting more keys than that is a
/// caller's broken count, and is refused rather than allowed to degrade.
pub struct PtrTable<K, V> {
    /// `(key, value)`; a null key is an empty slot. Empty, or a power of two.
    slots: Vec<(*const K, V)>,
    len: usize,
    /// The key count the table was sized for.
    limit: usize,
}

impl<K, V: Copy> PtrTable<K, V> {
    /// A table for up to `keys` keys, every slot holding `empty`. `None` when
    /// the allocation fails or the size overflows. Zero keys need no slots.
    pub fn with_keys(keys: usize, empty: V) -> Option<PtrTable<K, V>> {
        let mut slots = Vec::new();
        if keys > 0 {
            let cap = keys.checked_mul(2)?.checked_next_power_of_two()?.max(8);
            slots = try_vec_with_capacity(cap)?;
            slots.resize(cap, (core::ptr::null(), empty)); /* reserved above */
        }
        Some(PtrTable {
            slots,
            len: 0,
            limit: keys,
        })
    }

    #[inline]
    fn home(&self, key: *const K) -> usize {
        (ptr_hash(key) as usize) & (self.slots.len() - 1)
    }

    /// The slot holding `key`, or the empty slot where it would go.
    #[inline]
    fn probe(&self, key: *const K) -> usize {
        let mask = self.slots.len() - 1;
        let mut i = self.home(key);
        while !self.slots[i].0.is_null() && self.slots[i].0 != key {
            i = (i + 1) & mask;
        }
        i
    }

    /// Map a non-null `key` to `value` and return its slot. A key already
    /// present keeps its first value. `None` for a null key, or past the count
    /// the table was sized for.
    pub fn insert(&mut self, key: *const K, value: V) -> Option<usize> {
        if key.is_null() || self.slots.is_empty() {
            return None;
        }
        let i = self.probe(key);
        if self.slots[i].0.is_null() {
            if self.len == self.limit {
                return None;
            }
            self.slots[i] = (key, value);
            self.len += 1;
        }
        Some(i)
    }

    /// The value in slot `slot`, as [`insert`](Self::insert) returned it.
    #[inline]
    pub fn slot_mut(&mut self, slot: usize) -> &mut V {
        &mut self.slots[slot].1
    }

    /// The value `key` maps to.
    pub fn get(&self, key: *const K) -> Option<V> {
        if key.is_null() || self.slots.is_empty() {
            return None;
        }
        let (k, v) = self.slots[self.probe(key)];
        (!k.is_null()).then_some(v)
    }
}
