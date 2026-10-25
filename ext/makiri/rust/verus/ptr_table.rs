//! Verus proofs over `src/ptr_table.rs`: linear probing always stops, and
//! the tables keep the free slot that makes it stop.
//!
//! `probe` loops until it meets the key or an empty slot, so a table with no
//! empty slot would hang its caller - and a hang is a denial of service. What
//! keeps one free is each table's load factor: `PtrTable` is sized at twice
//! its key count, `PtrMap` grows before it passes half. Proved here, over the
//! shipped code: `capacity_for`, `probe`, `lookup`, `PtrTable::with_keys` /
//! `insert` / `get` and `PtrMap::clear` / `get` / `insert` / `set` / `grow`
//! are copied, every line only Verus needs ends in `// [verus]`, and
//! `rake verus:sync` checks the rest of each body against the source.
//!
//! The axioms: a key's `==` is equality (`eq_is_equality`; the keys are
//! `*const T`, `NodeKey` and `Token`, all derived), std's
//! `checked_next_power_of_two`, and `falloc::try_vec_filled` (a `Vec` of
//! `len` copies of `value`, or `None`).

use vstd::prelude::*;
use vstd::set_lib::*;
use vstd::std_specs::cmp::PartialEqSpec;

verus! {

// Makiri builds for 64-bit targets only.
global size_of usize == 8;

pub trait TableKey: Copy + Eq + Sized {
    const EMPTY: Self;
    fn table_hash(self) -> u64;
}

/// `==` on `K` is equality, as a derived `PartialEq` has it.
pub open spec fn eq_is_equality<K: PartialEqSpec>() -> bool {
    &&& K::obeys_eq_spec()
    &&& forall|a: K, b: K| #[trigger] a.eq_spec(&b) <==> a == b
}

pub open spec fn is_pow2(n: usize) -> bool {
    n > 0 && n & sub(n, 1) == 0
}

pub assume_specification[ usize::checked_next_power_of_two ](x: usize) -> (r: Option<usize>)
    ensures
        r.is_some() ==> is_pow2(r.unwrap()) && r.unwrap() >= x,
;

proof fn lemma_and_mask(x: usize, n: usize)
    requires
        is_pow2(n),
    ensures
        x & sub(n, 1) == x % n,
        x & sub(n, 1) < n,
{
    assert(x & sub(n, 1) == x % n) by (bit_vector)
        requires
            n > 0 && n & sub(n, 1) == 0;
    assert(x & sub(n, 1) < n) by (bit_vector)
        requires
            n > 0 && n & sub(n, 1) == 0;
}

proof fn lemma_and_mask_all(n: usize)
    requires
        is_pow2(n),
    ensures
        forall|x: usize| #![trigger x & sub(n, 1)] x & sub(n, 1) == x % n && x & sub(n, 1) < n,
{
    assert forall|x: usize| #![trigger x & sub(n, 1)] x & sub(n, 1) == x % n && x & sub(n, 1)
        < n by {
        lemma_and_mask(x, n);
    }
}

/// A power of two that fits a `usize` is at most half its range, so
/// doubling a count within it cannot overflow.
proof fn lemma_pow2_bound(n: usize)
    requires
        is_pow2(n),
    ensures
        n <= 0x8000_0000_0000_0000usize,
{
    assert(n <= 0x8000_0000_0000_0000usize) by (bit_vector)
        requires
            n > 0 && n & sub(n, 1) == 0;
}

broadcast use vstd::set::group_set_lemmas;

/// The occupied slots.
pub open spec fn occ<K: TableKey, V>(s: Seq<(K, V)>) -> Set<int> {
    set_int_range(0, s.len() as int).filter(|i: int| s[i].0 != K::EMPTY)
}

/// The occupied slots below `j`.
pub open spec fn occ_below<K: TableKey, V>(s: Seq<(K, V)>, j: int) -> Set<int> {
    set_int_range(0, s.len() as int).filter(|i: int| i < j && s[i].0 != K::EMPTY)
}

pub open spec fn has_free<K: TableKey, V>(s: Seq<(K, V)>) -> bool {
    exists|j: int| 0 <= j < s.len() && (#[trigger] s[j]).0 == K::EMPTY
}

proof fn lemma_occ_finite<K: TableKey, V>(s: Seq<(K, V)>, j: int)
    ensures
        occ(s).len() <= s.len(),
        occ_below(s, j).subset_of(occ(s)),
        occ_below(s, j).len() <= occ(s).len(),
{
    let r = set_int_range(0, s.len() as int);
    lemma_int_range(0, s.len() as int);
    assert(occ(s).subset_of(r));
    lemma_len_subset(occ(s), r);
    assert(occ_below(s, j).subset_of(occ(s)));
    lemma_len_subset(occ_below(s, j), occ(s));
}

/// Fewer occupied slots than slots: one is free.
proof fn lemma_free<K: TableKey, V>(s: Seq<(K, V)>)
    requires
        occ(s).len() < s.len(),
    ensures
        has_free(s),
{
    lemma_occ_finite(s, 0);
    if !has_free(s) {
        let r = set_int_range(0, s.len() as int);
        lemma_int_range(0, s.len() as int);
        assert(r.subset_of(occ(s)));
        lemma_len_subset(r, occ(s));
        assert(false);
    }
}

/// Writing `(key, v)` with `key` not EMPTY into slot `i`: the slot joins the
/// occupied ones, if it was free.
proof fn lemma_occ_write<K: TableKey, V>(s: Seq<(K, V)>, i: int, key: K, v: V)
    requires
        0 <= i < s.len(),
        key != K::EMPTY,
    ensures
        occ(s.update(i, (key, v))) == occ(s).insert(i),
        s[i].0 == K::EMPTY ==> occ(s.update(i, (key, v))).len() == occ(s).len() + 1,
        s[i].0 != K::EMPTY ==> occ(s.update(i, (key, v))).len() == occ(s).len(),
{
    lemma_occ_finite(s, 0);
    assert(occ(s.update(i, (key, v))) =~= occ(s).insert(i));
}

/// A table probe can run on: a power of two with a free slot.
pub open spec fn probeable<K: TableKey, V>(s: Seq<(K, V)>) -> bool {
    is_pow2(s.len() as usize) && s.len() <= usize::MAX && has_free(s)
}

pub fn capacity_for(keys: usize) -> (r: Option<usize>)
    ensures // [verus]
        r.is_some() ==> is_pow2(r.unwrap()) && r.unwrap() >= 8 && r.unwrap() >= 2 * keys, // [verus]
{
    proof { assert(is_pow2(8usize)) by (compute); } // [verus]
    Some(keys.checked_mul(2)?.checked_next_power_of_two()?.max(8))
}

fn probe<K: TableKey + PartialEqSpec, V>(slots: &[(K, V)], key: K) -> (r: usize)
    requires // [verus]
        eq_is_equality::<K>(), // [verus]
        probeable(slots@), // [verus]
    ensures // [verus]
        r < slots.len(), // [verus]
        slots[r as int].0 == K::EMPTY || slots[r as int].0 == key, // [verus]
{
    let mask = slots.len() - 1;
    proof { lemma_and_mask_all(slots.len()); } // [verus]
    let mut i = (key.table_hash() as usize) & mask;
    let ghost n = slots.len() as int; // [verus]
    let ghost h = i as int; // [verus]
    let ghost j = choose|j: int| 0 <= j < n && (#[trigger] slots@[j]).0 == K::EMPTY; // [verus]
    let ghost d = (j - h) % n; // [verus]
    let ghost mut steps: int = 0; // [verus]
    proof { // [verus]
        vstd::arithmetic::div_mod::lemma_small_mod(h as nat, n as nat); // [verus]
        vstd::arithmetic::div_mod::lemma_mod_bound(j - h, n); // [verus]
        assert((h + d) % n == j) by { // [verus]
            vstd::arithmetic::div_mod::lemma_add_mod_noop(h, j - h, n); // [verus]
            vstd::arithmetic::div_mod::lemma_small_mod(h as nat, n as nat); // [verus]
            vstd::arithmetic::div_mod::lemma_small_mod(j as nat, n as nat); // [verus]
        } // [verus]
    } // [verus]
    while slots[i].0 != K::EMPTY && slots[i].0 != key
        invariant // [verus]
            eq_is_equality::<K>(), // [verus]
            n == slots.len(), // [verus]
            mask == n - 1, // [verus]
            is_pow2(slots.len()), // [verus]
            0 <= h < n, // [verus]
            0 <= j < n, // [verus]
            slots@[j].0 == K::EMPTY, // [verus]
            0 <= d < n, // [verus]
            (h + d) % n == j, // [verus]
            0 <= steps <= d, // [verus]
            i < n, // [verus]
            i as int == (h + steps) % n, // [verus]
        decreases d - steps, // [verus]
    {
        proof { // [verus]
            if steps == d { // [verus]
                assert(i as int == j); // [verus]
                assert(false); // [verus]
            } // [verus]
            lemma_and_mask((i + 1) as usize, slots.len()); // [verus]
            vstd::arithmetic::div_mod::lemma_add_mod_noop_right(1, h + steps, n); // [verus]
        } // [verus]
        i = (i + 1) & mask;
        proof { steps = steps + 1; } // [verus]
    }
    i
}

fn lookup<K: TableKey + PartialEqSpec, V: Copy>(slots: &[(K, V)], key: K) -> (r: Option<V>)
    requires // [verus]
        eq_is_equality::<K>(), // [verus]
        slots.len() == 0 || probeable(slots@), // [verus]
    ensures // [verus]
        r.is_some() ==> exists|i: int| 0 <= i < slots.len() && (#[trigger] slots@[i]) == (key, r.unwrap()), // [verus]
{
    if key == K::EMPTY || slots.is_empty() {
        return None;
    }
    let (k, v) = slots[probe(slots, key)];
    if k != K::EMPTY {
        Some(v)
    } else {
        None
    }
}

pub mod falloc {
    use vstd::prelude::*;

    /// `src/falloc/mod.rs`'s, by its contract: `len` copies of `value`.
    #[verifier::external_body]
    pub fn try_vec_filled<T: Copy>(len: usize, value: T) -> (r: Option<Vec<T>>)
        ensures
            r.is_some() ==> r.unwrap()@.len() == len && forall|i: int|
                0 <= i < len ==> #[trigger] r.unwrap()@[i] == value,
    {
        unimplemented!()
    }

    pub trait Oom: Sized {
        fn oom() -> Self;
    }

    pub trait OomOption<T> {
        fn or_oom<E: Oom>(self) -> (r: Result<T, E>);
    }

    impl<T> OomOption<T> for Option<T> {
        #[verifier::external_body]
        fn or_oom<E: Oom>(self) -> (r: Result<T, E>)
            ensures
                self.is_some() <==> r.is_ok(),
                self.is_some() ==> r == Ok::<T, E>(self.unwrap()),
        {
            unimplemented!()
        }
    }
}

use falloc::OomOption;

/// The fields are the source's; public here so the specs below can name them.
#[verifier::reject_recursive_types(K)]
#[verifier::reject_recursive_types(V)]
pub struct PtrTable<K, V> {
    pub slots: Vec<(K, V)>,
    pub len: usize,
    pub limit: usize,
}

impl<K: TableKey + PartialEqSpec, V: Copy> PtrTable<K, V> {
    /// The invariant: no slots and no keys, or a power of two at least twice
    /// the key limit, with no more keys in it than `len`, which is within the
    /// limit - so one slot is always free.
    pub open spec fn wf(&self) -> bool {
        &&& self.len <= self.limit
        &&& occ(self.slots@).len() <= self.len
        &&& self.slots.len() == 0 || (is_pow2(self.slots.len()) && 2 * self.limit
            <= self.slots.len())
    }

    proof fn lemma_probeable(&self)
        requires
            self.wf(),
            self.len < self.limit,
            self.slots.len() > 0,
        ensures
            probeable(self.slots@),
    {
        lemma_free(self.slots@);
    }

    pub fn with_keys(keys: usize, empty: V) -> (r: Option<PtrTable<K, V>>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
        ensures // [verus]
            r.is_some() ==> r.unwrap().wf() && r.unwrap().limit == keys, // [verus]
    {
        let mut slots = Vec::new();
        if keys > 0 {
            let cap = capacity_for(keys)?;
            slots = crate::falloc::try_vec_filled(cap, (K::EMPTY, empty))?;
            assert(occ(slots@) =~= Set::<int>::empty()); // [verus]
        }
        assert(occ(slots@) =~= Set::<int>::empty()); // [verus]
        Some(PtrTable {
            slots,
            len: 0,
            limit: keys,
        })
    }

    pub fn insert(&mut self, key: K, value: V) -> (r: Option<usize>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).limit == old(self).limit, // [verus]
            r.is_some() ==> r.unwrap() < final(self).slots.len() && final(self).slots@[r.unwrap() as int].0 == key, // [verus]
    {
        if key == K::EMPTY || self.slots.is_empty() {
            return None;
        }
        proof { // [verus]
            lemma_occ_finite(self.slots@, 0); // [verus]
            lemma_free(self.slots@); // [verus]
        } // [verus]
        let i = probe(&self.slots, key);
        if self.slots[i].0 == K::EMPTY {
            if self.len == self.limit {
                return None;
            }
            proof { lemma_occ_write(self.slots@, i as int, key, value); } // [verus]
            self.slots[i] = (key, value);
            self.len += 1;
        }
        Some(i)
    }

    pub fn get(&self, key: K) -> (r: Option<V>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            self.wf(), // [verus]
    {
        proof { // [verus]
            if self.slots.len() > 0 { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
        } // [verus]
        lookup(&self.slots, key)
    }
}

#[derive(PartialEq, Eq)]
pub struct InsertRefused;

impl falloc::Oom for InsertRefused {
    fn oom() -> Self {
        InsertRefused
    }
}

/// The fields are the source's; public here so the specs below can name them.
#[verifier::reject_recursive_types(K)]
#[verifier::reject_recursive_types(V)]
pub struct PtrMap<K, V> {
    pub slots: Vec<(K, V)>,
    pub len: usize,
}

impl<K: TableKey + PartialEqSpec, V: Copy + Default> PtrMap<K, V> {
    const MIN_SLOTS: usize = 64;

    /// The invariant: no slots and no keys, or a power of two at least twice
    /// `len`, with no more keys in it than `len` - so one slot is always free.
    pub open spec fn wf(&self) -> bool {
        &&& occ(self.slots@).len() <= self.len
        &&& self.slots.len() == 0 ==> self.len == 0
        &&& self.slots.len() > 0 ==> is_pow2(self.slots.len()) && 2 * self.len <= self.slots.len()
    }

    pub fn clear(&mut self)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
    {
        if self.len != 0 {
            let mut j = 0;
            while j < self.slots.len()
                invariant // [verus]
                    self.slots.len() == old(self).slots.len(), // [verus]
                    old(self).wf(), // [verus]
                    j <= self.slots.len(), // [verus]
                    forall|k: int| 0 <= k < j ==> (#[trigger] self.slots@[k]).0 == K::EMPTY, // [verus]
                decreases self.slots.len() - j, // [verus]
            {
                self.slots[j] = (K::EMPTY, V::default());
                j += 1;
            }
            assert(occ(self.slots@) =~= Set::<int>::empty()); // [verus]
            self.len = 0;
        }
    }

    pub fn get(&self, key: K) -> (r: Option<V>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            self.wf(), // [verus]
    {
        proof { // [verus]
            if self.slots.len() > 0 { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_pow2_bound(self.slots.len()); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
        } // [verus]
        lookup(&self.slots, key)
    }

    pub fn insert(&mut self, key: K, value: V) -> (r: Result<(), InsertRefused>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
    {
        if key == K::EMPTY {
            return Err(InsertRefused);
        }
        if !self.slots.is_empty() {
            proof { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_pow2_bound(self.slots.len()); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
            let i = probe(&self.slots, key);
            if self.slots[i].0 != K::EMPTY {
                return Ok(());
            }
            if (self.len + 1) * 2 <= self.slots.len() {
                proof { lemma_occ_write(self.slots@, i as int, key, value); } // [verus]
                self.slots[i] = (key, value);
                self.len += 1;
                return Ok(());
            }
        }
        self.grow()?;
        proof { // [verus]
            lemma_occ_finite(self.slots@, 0); // [verus]
            lemma_free(self.slots@); // [verus]
        } // [verus]
        let i = probe(&self.slots, key);
        proof { lemma_occ_write(self.slots@, i as int, key, value); } // [verus]
        self.slots[i] = (key, value);
        self.len += 1;
        Ok(())
    }

    pub fn set(&mut self, key: K, value: V) -> (r: Result<(), InsertRefused>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
    {
        if key != K::EMPTY && !self.slots.is_empty() {
            proof { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_pow2_bound(self.slots.len()); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
            let i = probe(&self.slots, key);
            if self.slots[i].0 == key {
                proof { lemma_occ_write(self.slots@, i as int, key, value); } // [verus]
                self.slots[i].1 = value;
                assert(self.slots@ == old(self).slots@.update(i as int, (key, value))); // [verus]
                return Ok(());
            }
        }
        self.insert(key, value)
    }

    /// Doubles the table (or makes the first one): afterwards a power of two
    /// past twice `len`, holding at most `len` keys.
    fn grow(&mut self) -> (r: Result<(), InsertRefused>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).len == old(self).len, // [verus]
            occ(final(self).slots@).len() <= final(self).len, // [verus]
            r.is_ok() ==> is_pow2(final(self).slots.len()) && 2 * (final(self).len + 1) <= final(self).slots.len(), // [verus]
            r.is_err() ==> final(self).wf(), // [verus]
    {
        proof { // [verus]
            lemma_occ_finite(self.slots@, 0); // [verus]
            if self.slots.len() > 0 { lemma_pow2_bound(self.slots.len()); } // [verus]
        } // [verus]
        let cap = match self.slots.len() {
            0 => Self::MIN_SLOTS,
            n => n.checked_mul(2).ok_or(InsertRefused)?,
        };
        assert(is_pow2(cap)) by { // [verus]
            if self.slots.len() > 0 { // [verus]
                let n = self.slots.len(); // [verus]
                assert(mul(n, 2) > 0 && mul(n, 2) & sub(mul(n, 2), 1) == 0) by (bit_vector) // [verus]
                    requires n > 0 && n & sub(n, 1) == 0 && n < 0x8000_0000_0000_0000usize; // [verus]
                assert(cap == mul(n, 2)); // [verus]
            } else { // [verus]
                assert(is_pow2(64usize)) by (compute); // [verus]
            } // [verus]
        } // [verus]
        let mut slots: Vec<(K, V)> =
            crate::falloc::try_vec_filled(cap, (K::EMPTY, V::default())).or_oom()?;
        assert(occ(slots@) =~= Set::<int>::empty()); // [verus]
        let mut j = 0;
        while j < self.slots.len()
            invariant // [verus]
                eq_is_equality::<K>(), // [verus]
                old(self).wf(), // [verus]
                self.slots@ == old(self).slots@, // [verus]
                self.len == old(self).len, // [verus]
                slots.len() == cap, // [verus]
                is_pow2(cap), // [verus]
                self.slots.len() < cap, // [verus]
                j <= self.slots.len(), // [verus]
                occ(slots@).len() <= occ_below(self.slots@, j as int).len(), // [verus]
            decreases self.slots.len() - j, // [verus]
        {
            let (k, v) = self.slots[j];
            proof { // [verus]
                lemma_occ_finite(self.slots@, j as int); // [verus]
                lemma_occ_finite(self.slots@, j as int + 1); // [verus]
                if k != K::EMPTY { // [verus]
                    assert(occ_below(self.slots@, j as int + 1) =~= occ_below(self.slots@, j as int).insert(j as int)); // [verus]
                } else { // [verus]
                    assert(occ_below(self.slots@, j as int + 1) =~= occ_below(self.slots@, j as int)); // [verus]
                } // [verus]
            } // [verus]
            if k != K::EMPTY {
                proof { // [verus]
                    lemma_occ_finite(slots@, 0); // [verus]
                    lemma_free(slots@); // [verus]
                } // [verus]
                let i = probe(&slots, k);
                proof { lemma_occ_write(slots@, i as int, k, v); } // [verus]
                slots[i] = (k, v);
            }
            j += 1;
        }
        proof { // [verus]
            lemma_occ_finite(self.slots@, j as int); // [verus]
            assert(occ_below(self.slots@, j as int) =~= occ(self.slots@)); // [verus]
        } // [verus]
        self.slots = slots;
        Ok(())
    }
}

} // verus!

fn main() {}
