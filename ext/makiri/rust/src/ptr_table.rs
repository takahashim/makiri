//! Pointer-keyed tables: the one pointer hash, and the two open-addressing
//! tables every pointer- or token-keyed index in the crate is built on.
//!
//! Every such structure - the XPath string-value cache, the document-order
//! index, the `//tag[N]` per-parent count, the DOM and text indexes, the NodeSet
//! set operations - hashes with [`ptr_hash`] / [`mix64`]. They live here, below
//! all of them, so no index reaches into another layer's internals for a hash
//! function, and each probe loop - with the load-factor argument that makes it
//! terminate - is written once.
//!
//! - [`PtrTable`] is sized once for a key count known before it is filled, and
//!   never grows. Sizing once is what makes a build fail-closed: it gets its
//!   whole table or none.
//! - [`PtrMap`] grows as it is filled, for a cache or an index whose size is not
//!   known up front. A failed growth leaves it as it was.
//!
//! Both keep their load factor at or below 1/2, which is what makes every probe
//! find a free slot or its key.
//!
//! Verus proves both, over a copy `rake verus:sync` holds to this file
//! (`verus/ptr_table.rs`): every probe stops, and each table is the map it
//! stands for - `get` answers `Some(value)` exactly for a key put in it, and
//! `grow` keeps every entry. `lookup` names its probe's slot (`let i`) so the
//! proof can.

#![forbid(unsafe_code)]

use crate::falloc::OomOption;

/// The MurmurHash3 fmix64 finalizer over a pointer value.
///
/// Heap addresses are not attacker-chosen the way strings are, so a
/// SipHash-strength hash buys nothing here and costs a lot on the hot paths.
#[inline]
pub fn ptr_hash<T>(p: *const T) -> u64 {
    mix64(p as usize as u64)
}

/// The finalizer itself, for a caller that already holds the address as an
/// integer (a `Hasher` fed through `write_usize`, a node token's word).
#[inline]
pub fn mix64(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// [`mix64`] as a `Hasher`, for a std `HashMap`/`HashSet` keyed by a pointer or
/// a node token - an address or index is already well distributed, so it only
/// needs mixing, and std's SipHash (right for attacker-chosen keys) measured
/// about a third of the throughput on the NodeSet difference operator.
#[derive(Default, Clone, Copy)]
pub struct MixHasher(u64);

impl core::hash::Hasher for MixHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    /// Only ever fed one pointer or token (`write_usize`), but stays correct
    /// for anything else.
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }

    fn write_usize(&mut self, n: usize) {
        self.0 = mix64(n as u64);
    }
}

/// What a table can key on: a hash, and one value no entry ever uses, which
/// marks an empty slot.
///
/// A raw pointer's empty value is null. A key that is not a pointer supplies
/// its own: a `Token` is a kind AND a word, so its empty value is the one kind
/// no node has (`Token::null()`), not a zero word - which is why the empty
/// marker belongs to the key type rather than to the table. (No real XML node
/// token is word 0 either: arena slot 0 is reserved.)
pub trait TableKey: Copy + Eq {
    /// The key of an empty slot.
    const EMPTY: Self;
    fn table_hash(self) -> u64;
}

impl<T> TableKey for *const T {
    const EMPTY: Self = core::ptr::null();
    #[inline]
    fn table_hash(self) -> u64 {
        ptr_hash(self)
    }
}

/// A node's identity as a table key: a word equal exactly when the nodes are,
/// and never 0 for a node, since 0 is the empty slot - which the tables refuse
/// as a key. An integer, not an address: nothing reads through it, and unlike
/// a raw pointer it is `Send`, so a table of keys can live in a process-global
/// (`css_cache`'s kept `Scratch`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NodeKey(usize);

impl NodeKey {
    /// The key for a node's word (its host's own non-zero identity).
    #[inline]
    pub const fn new(word: usize) -> NodeKey {
        NodeKey(word)
    }
}

impl TableKey for NodeKey {
    const EMPTY: Self = NodeKey(0);
    #[inline]
    fn table_hash(self) -> u64 {
        mix64(self.0 as u64)
    }
}

/// The slot count for `keys` keys at load factor <= 1/2, or None on overflow.
fn capacity_for(keys: usize) -> Option<usize> {
    Some(keys.checked_mul(2)?.checked_next_power_of_two()?.max(8))
}

/// Linear probing over `slots` (a power of two, with a free slot): the slot
/// holding `key`, or the empty one where it would go.
#[inline]
fn probe<K: TableKey, V>(slots: &[(K, V)], key: K) -> usize {
    let mask = slots.len() - 1;
    let mut i = (key.table_hash() as usize) & mask;
    while slots[i].0 != K::EMPTY && slots[i].0 != key {
        i = (i + 1) & mask;
    }
    i
}

/// The value `key` maps to in `slots` (empty, or as [`probe`] requires).
#[inline]
fn lookup<K: TableKey, V: Copy>(slots: &[(K, V)], key: K) -> Option<V> {
    if key == K::EMPTY || slots.is_empty() {
        return None;
    }
    let i = probe(slots, key);
    let (k, v) = slots[i];
    if k != K::EMPTY {
        Some(v)
    } else {
        None
    }
}

/// A key -> `V` table for a key count known before it is filled.
///
/// Insert-only (there are no tombstones), sized at load factor <= 1/2 for the
/// count it was built for, and never grown: inserting more keys than that is a
/// caller's broken count, and is refused rather than allowed to degrade.
pub struct PtrTable<K, V> {
    /// `(key, value)`; `K::EMPTY` is an empty slot. Empty, or a power of two.
    slots: Vec<(K, V)>,
    len: usize,
    /// The key count the table was sized for.
    limit: usize,
}

impl<K: TableKey, V: Copy> PtrTable<K, V> {
    /// A table for up to `keys` keys, every slot holding `empty`. `None` when
    /// the allocation fails or the size overflows. Zero keys need no slots.
    pub fn with_keys(keys: usize, empty: V) -> Option<PtrTable<K, V>> {
        let mut slots = Vec::new();
        if keys > 0 {
            let cap = capacity_for(keys)?;
            slots = crate::falloc::try_vec_filled(cap, (K::EMPTY, empty))?;
        }
        Some(PtrTable {
            slots,
            len: 0,
            limit: keys,
        })
    }

    /// Map `key` to `value` and return its slot. A key already present keeps
    /// its first value. `None` for the empty key, or past the count the table
    /// was sized for.
    pub fn insert(&mut self, key: K, value: V) -> Option<usize> {
        if key == K::EMPTY || self.slots.is_empty() {
            return None;
        }
        let i = probe(&self.slots, key);
        if self.slots[i].0 == K::EMPTY {
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
    pub fn get(&self, key: K) -> Option<V> {
        lookup(&self.slots, key)
    }
}

/// Why a [`PtrMap`] refused an insert: the empty key, or a growth that could
/// not allocate. Either way the map is as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertRefused;

impl crate::falloc::Oom for InsertRefused {
    #[inline]
    fn oom() -> Self {
        InsertRefused
    }
}

/// A key -> `V` map that grows as it is filled, for a cache or an index whose
/// size is not known up front. Insert-only, like [`PtrTable`].
pub struct PtrMap<K, V> {
    /// As [`PtrTable::slots`]; empty until the first insert.
    slots: Vec<(K, V)>,
    len: usize,
}

impl<K: TableKey, V: Copy + Default> Default for PtrMap<K, V> {
    fn default() -> Self {
        PtrMap::new()
    }
}

impl<K: TableKey, V: Copy + Default> PtrMap<K, V> {
    /// The smallest table a first insert allocates.
    const MIN_SLOTS: usize = 64;

    pub const fn new() -> PtrMap<K, V> {
        PtrMap {
            slots: Vec::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The slots allocated: 0 before the first insert.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Remove every entry, keeping the allocation for the next fill.
    pub fn clear(&mut self) {
        if self.len != 0 {
            /* By index rather than `fill`, so `verus/ptr_table.rs` can copy
             * it as written. */
            let mut j = 0;
            while j < self.slots.len() {
                self.slots[j] = (K::EMPTY, V::default());
                j += 1;
            }
            self.len = 0;
        }
    }

    /// The value `key` maps to.
    #[inline]
    pub fn get(&self, key: K) -> Option<V> {
        lookup(&self.slots, key)
    }

    /// Map `key` to `value`; a key already present keeps its first value.
    /// `Err` for the empty key, or when growing could not allocate - in which
    /// case the map is exactly as it was.
    pub fn insert(&mut self, key: K, value: V) -> Result<(), InsertRefused> {
        if key == K::EMPTY {
            return Err(InsertRefused);
        }
        /* A key already present is a no-op, so it is looked for BEFORE growing:
         * growing first made a repeated key allocate, and fail on OOM, for a
         * write that changes nothing. When no growth is needed, the slot that
         * probe found is where the key goes. */
        if !self.slots.is_empty() {
            let i = probe(&self.slots, key);
            if self.slots[i].0 != K::EMPTY {
                return Ok(());
            }
            if (self.len + 1) * 2 <= self.slots.len() {
                self.slots[i] = (key, value);
                self.len += 1;
                return Ok(());
            }
        }
        self.grow()?;
        let i = probe(&self.slots, key);
        self.slots[i] = (key, value);
        self.len += 1;
        Ok(())
    }

    /// Map `key` to `value`, replacing the value a key already present has -
    /// for a cache whose entries are refined, where [`insert`](Self::insert)
    /// keeps the first. `Err` as `insert`'s; a replacement never allocates.
    pub fn set(&mut self, key: K, value: V) -> Result<(), InsertRefused> {
        if key != K::EMPTY && !self.slots.is_empty() {
            let i = probe(&self.slots, key);
            if self.slots[i].0 == key {
                self.slots[i].1 = value;
                return Ok(());
            }
        }
        self.insert(key, value)
    }

    /// Double the table (or make the first one), re-placing every entry. The
    /// new table is built whole before it replaces the old one.
    fn grow(&mut self) -> Result<(), InsertRefused> {
        let cap = match self.slots.len() {
            0 => Self::MIN_SLOTS,
            n => n.checked_mul(2).ok_or(InsertRefused)?,
        };
        let mut slots: Vec<(K, V)> =
            crate::falloc::try_vec_filled(cap, (K::EMPTY, V::default())).or_oom()?;
        /* By index rather than an iterator chain, so `verus/ptr_table.rs`
         * can copy it as written. */
        let mut j = 0;
        while j < self.slots.len() {
            let (k, v) = self.slots[j];
            if k != K::EMPTY {
                let i = probe(&slots, k);
                slots[i] = (k, v);
            }
            j += 1;
        }
        self.slots = slots;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map grown past several doublings keeps every key, `set` replaces a
    /// value where `insert` keeps the first, and `clear` empties it for a
    /// refill.
    #[test]
    fn a_map_keeps_its_keys_across_growth_and_clear() {
        let mut m: PtrMap<NodeKey, u32> = PtrMap::new();
        for k in 1..=1000usize {
            m.insert(NodeKey::new(k), k as u32).unwrap();
        }
        assert_eq!(m.len(), 1000);
        assert!(m.capacity() >= 2000 && m.capacity().is_power_of_two());
        assert!((1..=1000usize).all(|k| m.get(NodeKey::new(k)) == Some(k as u32)));
        assert_eq!(m.get(NodeKey::new(1001)), None);
        m.insert(NodeKey::new(7), 0).unwrap();
        assert_eq!(m.get(NodeKey::new(7)), Some(7));
        m.set(NodeKey::new(7), 70).unwrap();
        assert_eq!(m.get(NodeKey::new(7)), Some(70));
        assert_eq!(m.insert(NodeKey::EMPTY, 1), Err(InsertRefused));
        let cap = m.capacity();
        m.clear();
        assert_eq!((m.len(), m.capacity()), (0, cap));
        assert_eq!(m.get(NodeKey::new(7)), None);
        m.insert(NodeKey::new(7), 1).unwrap();
        assert_eq!(m.get(NodeKey::new(7)), Some(1));
    }

    /// A table takes the keys it was sized for and refuses one more.
    #[test]
    fn a_table_takes_exactly_its_keys() {
        let mut t: PtrTable<NodeKey, u32> = PtrTable::with_keys(10, 0).unwrap();
        for k in 1..=10usize {
            assert!(t.insert(NodeKey::new(k), k as u32).is_some());
        }
        assert_eq!(t.insert(NodeKey::new(11), 11), None);
        assert_eq!(t.insert(NodeKey::new(3), 99).map(|_| ()), Some(()));
        assert!((1..=10usize).all(|k| t.get(NodeKey::new(k)) == Some(k as u32)));
        assert!(PtrTable::<NodeKey, u32>::with_keys(0, 0)
            .unwrap()
            .get(NodeKey::new(1))
            .is_none());
    }
}
