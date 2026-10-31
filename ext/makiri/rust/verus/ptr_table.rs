//! Verus proofs over `engine/src/ptr_table.rs`: linear probing always stops, the
//! tables keep the free slot that makes it stop, and each table is the map
//! it stands for - `get` answers what was put, and only that.
//!
//! The map: `view` reads the slots as a `Map<K, V>`, and every operation is
//! specified on it - `PtrTable::insert` adds a key it does not hold (or
//! leaves the table as it is, a full one included), `PtrMap::insert` the same,
//! `set` adds or replaces, `clear` empties, `grow` keeps it, `get` answers
//! `Some(value)` exactly for a key in it. What makes `get` complete is two
//! conditions in each `wf`: `unique` (no key in two slots) and `clustered`
//! (every slot between a key's home and its slot is taken), so a probe that
//! meets an empty slot answers "absent" truly (`lemma_probe_finds`).
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
//! `*const T`, `NodeKey` and `Token`, all derived), a key's hash is a
//! function of the key alone (`TableKey::hash_spec`: the source's
//! `table_hash` impls, a pointer's address and two fields' mix, are taken to
//! be, not proved), std's `fill` (each element the value: a clone of a
//! `Copy` pair is the pair), std's
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
    /// What `table_hash` answers: a key's hash depends on the key alone.
    spec fn hash_spec(self) -> u64;
    fn table_hash(self) -> (r: u64)
        ensures
            r == self.hash_spec(),
    ;
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

/// `fill` writes `value` into every element: what a clone is for the one
/// type it fills here, a table's (key, value) pair of `Copy` types.
pub assume_specification<T: Clone>[ <[T]>::fill ](s: &mut [T], value: T)
    ensures
        final(s)@.len() == old(s)@.len(),
        forall|i: int| 0 <= i < final(s)@.len() ==> #[trigger] final(s)@[i] == value,
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

/* ---- what a table holds ---- */

/// Where a probe for `k` starts in a table of `n` slots.
pub open spec fn home<K: TableKey>(k: K, n: int) -> int {
    (k.hash_spec() as int) % n
}

/// How many steps past its home slot `i` is.
pub open spec fn dist(h: int, i: int, n: int) -> int {
    (i - h) % n
}

/// No key in two slots.
pub open spec fn unique<K: TableKey, V>(s: Seq<(K, V)>) -> bool {
    forall|i: int, j: int|
        0 <= i < s.len() && 0 <= j < s.len() && (#[trigger] s[i]).0 != K::EMPTY && s[i].0 == (
        #[trigger] s[j]).0 ==> i == j
}

/// Every slot between a key's home and the slot it is in is taken: what lets
/// a probe that meets an empty slot answer "absent".
pub open spec fn clustered<K: TableKey, V>(s: Seq<(K, V)>) -> bool {
    forall|i: int, d: int|
        0 <= i < s.len() && (#[trigger] s[i]).0 != K::EMPTY && 0 <= d < dist(
            home(s[i].0, s.len() as int),
            i,
            s.len() as int,
        ) ==> (#[trigger] s[(home(s[i].0, s.len() as int) + d) % (s.len() as int)]).0 != K::EMPTY
}

pub open spec fn has_key<K: TableKey, V>(s: Seq<(K, V)>, k: K) -> bool {
    k != K::EMPTY && exists|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == k
}

pub open spec fn has_key_below<K: TableKey, V>(s: Seq<(K, V)>, k: K, j: int) -> bool {
    k != K::EMPTY && exists|i: int| 0 <= i < s.len() && i < j && (#[trigger] s[i]).0 == k
}

pub open spec fn value_of<K: TableKey, V>(s: Seq<(K, V)>, k: K) -> V {
    s[choose|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == k].1
}

/// The table as the map it stands for.
pub open spec fn view<K: TableKey, V>(s: Seq<(K, V)>) -> Map<K, V> {
    Map::new(occ(s).map(|i: int| s[i].0), |k: K| value_of(s, k))
}

/// The map the slots below `j` stand for.
pub open spec fn view_below<K: TableKey, V>(s: Seq<(K, V)>, j: int) -> Map<K, V> {
    Map::new(occ_below(s, j).map(|i: int| s[i].0), |k: K| value_of(s, k))
}

proof fn lemma_view_dom<K: TableKey, V>(s: Seq<(K, V)>, j: int)
    ensures
        forall|k: K| #[trigger] view(s).contains_key(k) <==> has_key(s, k),
        forall|k: K| #[trigger] view_below(s, j).contains_key(k) <==> has_key_below(s, k, j),
        forall|k: K| #[trigger] view(s).contains_key(k) ==> view(s)[k] == value_of(s, k),
        forall|k: K| #[trigger] view_below(s, j).contains_key(k) ==> view_below(s, j)[k] == value_of(s, k),
{
    assert forall|k: K| #[trigger] view(s).contains_key(k) <==> has_key(s, k) by {
        if has_key(s, k) {
            let i = choose|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == k;
            assert(occ(s).contains(i));
        }
    }
    assert forall|k: K| #[trigger] view_below(s, j).contains_key(k) <==> has_key_below(s, k, j) by {
        if has_key_below(s, k, j) {
            let i = choose|i: int| 0 <= i < s.len() && i < j && (#[trigger] s[i]).0 == k;
            assert(occ_below(s, j).contains(i));
        }
    }
}

/// What a probe for `k` that answered `r` saw: it started at `k`'s home and
/// passed only taken slots holding other keys.
pub open spec fn probed<K: TableKey, V>(s: Seq<(K, V)>, k: K, r: int) -> bool {
    exists|t: int|
        #[trigger] path_clear(s, k, t) && 0 <= t < s.len() && r == (home(k, s.len() as int) + t) % (
        s.len() as int)
}

/// The first `t` slots from `k`'s home are taken, by other keys.
pub open spec fn path_clear<K: TableKey, V>(s: Seq<(K, V)>, k: K, t: int) -> bool {
    forall|d: int|
        0 <= d < t ==> (#[trigger] s[(home(k, s.len() as int) + d) % (s.len() as int)]).0
            != K::EMPTY && s[(home(k, s.len() as int) + d) % (s.len() as int)].0 != k
}

/// `x % n` for an `x` within one `n` of the range: no division needed.
proof fn lemma_mod_near(x: int, n: int)
    requires
        n > 0,
        -n <= x < 2 * n,
    ensures
        x % n == if x < 0 { x + n } else if x < n { x } else { x - n },
{
    if x < 0 {
        vstd::arithmetic::div_mod::lemma_small_mod((x + n) as nat, n as nat);
        vstd::arithmetic::div_mod::lemma_mod_add_multiples_vanish(x, n);
    } else if x < n {
        vstd::arithmetic::div_mod::lemma_small_mod(x as nat, n as nat);
    } else {
        vstd::arithmetic::div_mod::lemma_small_mod((x - n) as nat, n as nat);
        vstd::arithmetic::div_mod::lemma_mod_add_multiples_vanish(x - n, n);
    }
}

/// Walking `a` and `b` steps from `h` lands on the same slot only when
/// `a == b`, and `a` steps land `a` past `h`.
proof fn lemma_mod_walk(h: int, a: int, b: int, n: int)
    requires
        n > 0,
        0 <= h < n,
        0 <= a < n,
        0 <= b < n,
    ensures
        (h + a) % n == (h + b) % n ==> a == b,
        0 <= (h + a) % n < n,
        dist(h, (h + a) % n, n) == a,
{
    lemma_mod_near(h + a, n);
    lemma_mod_near(h + b, n);
    lemma_mod_near((h + a) % n - h, n);
}

/// From `h`, `dist(h, i, n)` steps reach `i`.
proof fn lemma_dist(h: int, i: int, n: int)
    requires
        n > 0,
        0 <= h < n,
        0 <= i < n,
    ensures
        0 <= dist(h, i, n) < n,
        (h + dist(h, i, n)) % n == i,
{
    lemma_mod_near(i - h, n);
    lemma_mod_near(h + dist(h, i, n), n);
}

proof fn lemma_home(k: impl TableKey, n: int)
    requires
        n > 0,
    ensures
        0 <= home(k, n) < n,
{
    vstd::arithmetic::div_mod::lemma_mod_bound(k.hash_spec() as int, n);
}

/// The one slot holding a key holds its value.
proof fn lemma_value_of<K: TableKey, V>(s: Seq<(K, V)>, i: int)
    requires
        unique(s),
        0 <= i < s.len(),
        s[i].0 != K::EMPTY,
    ensures
        has_key(s, s[i].0),
        value_of(s, s[i].0) == s[i].1,
{
    let j = choose|j: int| 0 <= j < s.len() && (#[trigger] s[j]).0 == s[i].0;
    assert(s[j].0 == s[i].0);
}

/// A probe over a clustered table of unique keys stops at the key's slot
/// whenever the key is there: an empty slot answers "absent" truly.
proof fn lemma_probe_finds<K: TableKey, V>(s: Seq<(K, V)>, k: K, r: int)
    requires
        s.len() > 0,
        clustered(s),
        unique(s),
        k != K::EMPTY,
        0 <= r < s.len(),
        probed(s, k, r),
        s[r].0 == K::EMPTY || s[r].0 == k,
    ensures
        has_key(s, k) <==> s[r].0 == k,
{
    let n = s.len() as int;
    let h = home(k, n);
    lemma_home(k, n);
    if has_key(s, k) {
        let i = choose|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == k;
        let t = choose|t: int|
            path_clear(s, k, t) && 0 <= t < s.len() && r == (h + t) % n;
        lemma_dist(h, i, n);
        let di = dist(h, i, n);
        lemma_mod_walk(h, t, di, n);
        if t < di {
            assert(s[(h + t) % n].0 != K::EMPTY);
            assert(s[r].0 == k);
            assert(r == i);
            assert(false);
        } else if t > di {
            assert(s[(h + di) % n].0 != k);
            assert(false);
        }
    }
}

/// A key not in the table, written into the empty slot its probe found: the
/// table stays clustered and unique, and stands for the map with it added.
proof fn lemma_insert_empty<K: TableKey, V>(s: Seq<(K, V)>, k: K, v: V, r: int)
    requires
        s.len() > 0,
        clustered(s),
        unique(s),
        k != K::EMPTY,
        0 <= r < s.len(),
        probed(s, k, r),
        s[r].0 == K::EMPTY,
    ensures
        !has_key(s, k),
        clustered(s.update(r, (k, v))),
        unique(s.update(r, (k, v))),
        view(s.update(r, (k, v))) == view(s).insert(k, v),
{
    let n = s.len() as int;
    let s2 = s.update(r, (k, v));
    lemma_probe_finds(s, k, r);
    let h = home(k, n);
    lemma_home(k, n);
    let t = choose|t: int|
        path_clear(s, k, t) && 0 <= t < s.len() && r == (h + t) % n;
    assert forall|i: int, j: int|
        0 <= i < s2.len() && 0 <= j < s2.len() && (#[trigger] s2[i]).0 != K::EMPTY && s2[i].0 == (
        #[trigger] s2[j]).0 implies i == j by {
        if i != r && j != r {
            assert(s[i] == s2[i] && s[j] == s2[j]);
        } else if i == r && j != r {
            assert(s[j].0 == k);
        } else if j == r && i != r {
            assert(s[i].0 == k);
        }
    }
    assert forall|i: int, d: int|
        0 <= i < s2.len() && (#[trigger] s2[i]).0 != K::EMPTY && 0 <= d < dist(
            home(s2[i].0, n),
            i,
            n,
        ) implies (#[trigger] s2[(home(s2[i].0, n) + d) % n]).0 != K::EMPTY by {
        let hi = home(s2[i].0, n);
        lemma_home(s2[i].0, n);
        lemma_mod_walk(hi, d, 0, n);
        let p = (hi + d) % n;
        if i == r {
            lemma_mod_walk(h, t, 0, n);
            assert(dist(h, r, n) == t);
            lemma_mod_walk(h, d, t, n);
            assert(s[(h + d) % n].0 != K::EMPTY);
        } else {
            assert(s[i] == s2[i]);
            assert(s[p].0 != K::EMPTY);
        }
    }
    assert forall|x: K| #[trigger] has_key(s2, x) == (has_key(s, x) || x == k) by {
        if has_key(s, x) {
            let i = choose|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == x;
            assert(i != r);
            assert(s2[i].0 == x);
        }
        if has_key(s2, x) && x != k {
            let i = choose|i: int| 0 <= i < s2.len() && (#[trigger] s2[i]).0 == x;
            assert(s[i].0 == x);
        }
        if x == k {
            assert(s2[r].0 == k);
        }
    }
    lemma_view_dom(s, 0);
    lemma_view_dom(s2, 0);
    assert forall|x: K| #[trigger] view(s2).contains_key(x) implies view(s2)[x] == view(s).insert(
        k,
        v,
    )[x] by {
        let i = choose|i: int| 0 <= i < s2.len() && (#[trigger] s2[i]).0 == x;
        lemma_value_of(s2, i);
        if x != k {
            assert(i != r);
            assert(s[i] == s2[i]);
            lemma_value_of(s, i);
        } else {
            assert(s2[r].0 == k);
            assert(i == r);
        }
    }
    assert(view(s2) =~= view(s).insert(k, v));
}

/// A new value for a key the slot `r` already holds.
proof fn lemma_overwrite<K: TableKey, V>(s: Seq<(K, V)>, k: K, v: V, r: int)
    requires
        clustered(s),
        unique(s),
        0 <= r < s.len(),
        s[r].0 == k,
        k != K::EMPTY,
    ensures
        clustered(s.update(r, (k, v))),
        unique(s.update(r, (k, v))),
        view(s.update(r, (k, v))) == view(s).insert(k, v),
{
    let s2 = s.update(r, (k, v));
    assert forall|i: int| 0 <= i < s.len() implies (#[trigger] s2[i]).0 == s[i].0 by {}
    assert forall|x: K| #[trigger] has_key(s2, x) == has_key(s, x) by {
        if has_key(s, x) {
            let i = choose|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == x;
            assert(s2[i].0 == x);
        }
        if has_key(s2, x) {
            let i = choose|i: int| 0 <= i < s2.len() && (#[trigger] s2[i]).0 == x;
            assert(s[i].0 == x);
        }
    }
    assert(unique(s2));
    assert(clustered(s2));
    lemma_value_of(s, r);
    lemma_view_dom(s, 0);
    lemma_view_dom(s2, 0);
    assert forall|x: K| #[trigger] view(s2).contains_key(x) implies view(s2)[x] == view(s).insert(
        k,
        v,
    )[x] by {
        let i = choose|i: int| 0 <= i < s2.len() && (#[trigger] s2[i]).0 == x;
        lemma_value_of(s2, i);
        if x != k {
            assert(i != r);
            assert(s[i] == s2[i]);
            lemma_value_of(s, i);
        } else {
            assert(s2[r].0 == k);
            assert(i == r);
        }
    }
    assert(view(s2) =~= view(s).insert(k, v));
}

/// A table of empty slots holds nothing.
proof fn lemma_all_empty<K: TableKey, V>(s: Seq<(K, V)>)
    requires
        forall|i: int| 0 <= i < s.len() ==> (#[trigger] s[i]).0 == K::EMPTY,
    ensures
        clustered(s),
        unique(s),
        view(s) == Map::<K, V>::empty(),
{
    assert(view(s) =~= Map::<K, V>::empty());
}

/// No occupied slot is no key.
proof fn lemma_occ_none<K: TableKey, V>(s: Seq<(K, V)>)
    requires
        occ(s).len() == 0,
    ensures
        forall|i: int| 0 <= i < s.len() ==> (#[trigger] s[i]).0 == K::EMPTY,
{
    lemma_occ_finite(s, 0);
    assert forall|i: int| 0 <= i < s.len() implies (#[trigger] s[i]).0 == K::EMPTY by {
        if s[i].0 != K::EMPTY {
            assert(occ(s).contains(i));
            lemma_len_subset(set![i], occ(s));
        }
    }
}

/// One more slot of the old table, rehashed: the map below `j + 1`.
proof fn lemma_view_below_step<K: TableKey, V>(s: Seq<(K, V)>, j: int)
    requires
        unique(s),
        0 <= j < s.len(),
    ensures
        s[j].0 != K::EMPTY ==> !has_key_below(s, s[j].0, j),
        view_below(s, j + 1) == if s[j].0 != K::EMPTY {
            view_below(s, j).insert(s[j].0, s[j].1)
        } else {
            view_below(s, j)
        },
{
    lemma_view_dom(s, j);
    lemma_view_dom(s, j + 1);
    if s[j].0 != K::EMPTY {
        if has_key_below(s, s[j].0, j) {
            let i = choose|i: int| 0 <= i < s.len() && i < j && (#[trigger] s[i]).0 == s[j].0;
            assert(false);
        }
        lemma_value_of(s, j);
        assert forall|x: K| #[trigger] has_key_below(s, x, j + 1) == (has_key_below(s, x, j) || x == s[j].0) by {
            if has_key_below(s, x, j + 1) && x != s[j].0 {
                let i = choose|i: int| 0 <= i < s.len() && i < j + 1 && (#[trigger] s[i]).0 == x;
                assert(i < j);
            }
            if x == s[j].0 {
                assert(s[j].0 == x);
            }
            if has_key_below(s, x, j) {
                let i = choose|i: int| 0 <= i < s.len() && i < j && (#[trigger] s[i]).0 == x;
                assert(i < j + 1);
            }
        }
        assert(view_below(s, j + 1) =~= view_below(s, j).insert(s[j].0, s[j].1));
    } else {
        assert forall|x: K| #[trigger] has_key_below(s, x, j + 1) == has_key_below(s, x, j) by {
            if has_key_below(s, x, j + 1) {
                let i = choose|i: int| 0 <= i < s.len() && i < j + 1 && (#[trigger] s[i]).0 == x;
                assert(i < j);
            }
            if has_key_below(s, x, j) {
                let i = choose|i: int| 0 <= i < s.len() && i < j && (#[trigger] s[i]).0 == x;
                assert(i < j + 1);
            }
        }
        assert(view_below(s, j + 1) =~= view_below(s, j));
    }
}

proof fn lemma_view_below_ends<K: TableKey, V>(s: Seq<(K, V)>)
    ensures
        view_below(s, 0) == Map::<K, V>::empty(),
        view_below(s, s.len() as int) == view(s),
{
    lemma_view_dom(s, 0);
    lemma_view_dom(s, s.len() as int);
    assert forall|x: K| #[trigger] has_key_below(s, x, s.len() as int) == has_key(s, x) by {
        if has_key(s, x) {
            let i = choose|i: int| 0 <= i < s.len() && (#[trigger] s[i]).0 == x;
            assert(i < s.len());
        }
    }
    assert(view_below(s, 0) =~= Map::<K, V>::empty());
    assert(view_below(s, s.len() as int) =~= view(s));
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
    // [canary]
    Some(keys.checked_mul(2)?.checked_next_power_of_two()?.max(8))
}

fn probe<K: TableKey + PartialEqSpec, V>(slots: &[(K, V)], key: K) -> (r: usize)
    requires // [verus]
        eq_is_equality::<K>(), // [verus]
        probeable(slots@), // [verus]
    ensures // [verus]
        r < slots.len(), // [verus]
        slots[r as int].0 == K::EMPTY || slots[r as int].0 == key, // [verus]
        probed(slots@, key, r as int), // [verus]
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
            h == home(key, n), // [verus]
            path_clear(slots@, key, steps), // [verus]
        decreases d - steps, // [verus]
    {
        // [canary]
        proof { // [verus]
            if steps == d { // [verus]
                assert(i as int == j); // [verus]
                assert(false); // [verus]
            } // [verus]
            lemma_and_mask((i + 1) as usize, slots.len()); // [verus]
            vstd::arithmetic::div_mod::lemma_add_mod_noop_right(1, h + steps, n); // [verus]
        } // [verus]
        i = (i + 1) & mask;
        proof { // [verus]
            assert(path_clear(slots@, key, steps + 1)) by { // [verus]
                assert forall|e: int| 0 <= e < steps + 1 implies (#[trigger] slots@[(h + e) % n]).0 != K::EMPTY && slots@[(h + e) % n].0 != key by { // [verus]
                    if e < steps { assert(slots@[(h + e) % n].0 != K::EMPTY); } // [verus]
                } // [verus]
            } // [verus]
            steps = steps + 1; // [verus]
        } // [verus]
    }
    assert(path_clear(slots@, key, steps)); // [verus]
    // [canary]
    i
}

fn lookup<K: TableKey + PartialEqSpec, V: Copy>(slots: &[(K, V)], key: K) -> (r: Option<V>)
    requires // [verus]
        eq_is_equality::<K>(), // [verus]
        slots.len() == 0 || probeable(slots@), // [verus]
        clustered(slots@), // [verus]
        unique(slots@), // [verus]
    ensures // [verus]
        r == if view(slots@).contains_key(key) { Some(view(slots@)[key]) } else { None::<V> }, // [verus]
{
    proof { lemma_view_dom(slots@, 0); } // [verus]
    if key == K::EMPTY || slots.is_empty() {
        return None;
    }
    let i = probe(slots, key);
    let (k, v) = slots[i];
    proof { lemma_probe_finds(slots@, key, i as int); } // [verus]
    if k != K::EMPTY {
        // [canary]
        proof { lemma_value_of(slots@, i as int); } // [verus]
        Some(v)
    } else {
        None
    }
}

pub mod falloc {
    use vstd::prelude::*;

    /// `falloc/src/lib.rs`'s, by its contract: `len` copies of `value`.
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
        &&& clustered(self.slots@)
        &&& unique(self.slots@)
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
            r.is_some() ==> view(r.unwrap().slots@) == Map::<K, V>::empty(), // [verus]
    {
        let mut slots = Vec::new();
        if keys > 0 {
            let cap = capacity_for(keys)?;
            slots = falloc::try_vec_filled(cap, (K::EMPTY, empty))?;
            // [canary]
            assert(occ(slots@) =~= Set::<int>::empty()); // [verus]
        }
        assert(occ(slots@) =~= Set::<int>::empty()); // [verus]
        proof { lemma_all_empty(slots@); } // [verus]
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
            r.is_some() ==> view(final(self).slots@) == if view(old(self).slots@).contains_key(key) { // [verus]
                view(old(self).slots@) } else { view(old(self).slots@).insert(key, value) }, // [verus]
            r.is_none() ==> final(self).slots@ == old(self).slots@, // [verus]
    {
        if key == K::EMPTY || self.slots.is_empty() {
            return None;
        }
        proof { // [verus]
            lemma_occ_finite(self.slots@, 0); // [verus]
            lemma_free(self.slots@); // [verus]
        } // [verus]
        let i = probe(&self.slots, key);
        proof { lemma_probe_finds(self.slots@, key, i as int); lemma_view_dom(self.slots@, 0); } // [verus]
        if self.slots[i].0 == K::EMPTY {
            if self.len == self.limit {
                return None;
            }
            // [canary]
            proof { lemma_occ_write(self.slots@, i as int, key, value); lemma_insert_empty(self.slots@, key, value, i as int); } // [verus]
            self.slots[i] = (key, value);
            self.len += 1;
        }
        // [canary]
        Some(i)
    }

    pub fn get(&self, key: K) -> (r: Option<V>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            self.wf(), // [verus]
        ensures // [verus]
            r == if view(self.slots@).contains_key(key) { Some(view(self.slots@)[key]) } else { None::<V> }, // [verus]
    {
        proof { // [verus]
            if self.slots.len() > 0 { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
        } // [verus]
        // [canary]
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
        &&& clustered(self.slots@)
        &&& unique(self.slots@)
    }

    /// The map the table stands for.
    pub open spec fn map(&self) -> Map<K, V> {
        view(self.slots@)
    }

    pub fn clear(&mut self)
        requires // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            final(self).map() == Map::<K, V>::empty(), // [verus]
    {
        if self.len != 0 {
            // [canary]
            self.slots.fill((K::EMPTY, V::default()));
            assert(occ(self.slots@) =~= Set::<int>::empty()); // [verus]
            proof { lemma_all_empty(self.slots@); } // [verus]
            self.len = 0;
        }
        proof { lemma_occ_finite(self.slots@, 0); lemma_occ_none(self.slots@); lemma_all_empty(self.slots@); } // [verus]
    }

    pub fn get(&self, key: K) -> (r: Option<V>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            self.wf(), // [verus]
        ensures // [verus]
            r == if self.map().contains_key(key) { Some(self.map()[key]) } else { None::<V> }, // [verus]
    {
        proof { // [verus]
            if self.slots.len() > 0 { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_pow2_bound(self.slots.len()); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
        } // [verus]
        // [canary]
        lookup(&self.slots, key)
    }

    pub fn insert(&mut self, key: K, value: V) -> (r: Result<(), InsertRefused>)
        requires // [verus]
            eq_is_equality::<K>(), // [verus]
            old(self).wf(), // [verus]
        ensures // [verus]
            final(self).wf(), // [verus]
            r.is_ok() ==> final(self).map() == if old(self).map().contains_key(key) { // [verus]
                old(self).map() } else { old(self).map().insert(key, value) }, // [verus]
            r.is_err() ==> final(self).map() == old(self).map(), // [verus]
    {
        proof { lemma_view_dom(self.slots@, 0); } // [verus]
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
            proof { lemma_probe_finds(self.slots@, key, i as int); } // [verus]
            if self.slots[i].0 != K::EMPTY {
                return Ok(());
            }
            if (self.len + 1) * 2 <= self.slots.len() {
                // [canary]
                proof { lemma_occ_write(self.slots@, i as int, key, value); lemma_insert_empty(self.slots@, key, value, i as int); } // [verus]
                self.slots[i] = (key, value);
                self.len += 1;
                return Ok(());
            }
        }
        let ghost absent = !old(self).map().contains_key(key); // [verus]
        proof { if self.slots.len() == 0 { assert(absent); } } // [verus]
        self.grow()?;
        proof { // [verus]
            lemma_occ_finite(self.slots@, 0); // [verus]
            lemma_free(self.slots@); // [verus]
            lemma_view_dom(self.slots@, 0); // [verus]
        } // [verus]
        let i = probe(&self.slots, key);
        proof { lemma_probe_finds(self.slots@, key, i as int); } // [verus]
        // [canary]
        proof { lemma_occ_write(self.slots@, i as int, key, value); lemma_insert_empty(self.slots@, key, value, i as int); } // [verus]
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
            r.is_ok() ==> final(self).map() == old(self).map().insert(key, value), // [verus]
            r.is_err() ==> final(self).map() == old(self).map(), // [verus]
    {
        proof { lemma_view_dom(self.slots@, 0); } // [verus]
        if key != K::EMPTY && !self.slots.is_empty() {
            proof { // [verus]
                lemma_occ_finite(self.slots@, 0); // [verus]
                lemma_pow2_bound(self.slots.len()); // [verus]
                lemma_free(self.slots@); // [verus]
            } // [verus]
            let i = probe(&self.slots, key);
            proof { lemma_probe_finds(self.slots@, key, i as int); } // [verus]
            if self.slots[i].0 == key {
                proof { lemma_occ_write(self.slots@, i as int, key, value); lemma_overwrite(self.slots@, key, value, i as int); } // [verus]
                // [canary]
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
            r.is_ok() ==> clustered(final(self).slots@) && unique(final(self).slots@), // [verus]
            r.is_ok() ==> final(self).map() == old(self).map(), // [verus]
            r.is_err() ==> final(self).slots@ == old(self).slots@, // [verus]
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
            falloc::try_vec_filled(cap, (K::EMPTY, V::default())).or_oom()?;
        assert(occ(slots@) =~= Set::<int>::empty()); // [verus]
        proof { lemma_all_empty(slots@); lemma_view_below_ends(self.slots@); } // [verus]
        for &(k, v) in
            iter: // [verus]
            self.slots.iter()
            invariant // [verus]
                eq_is_equality::<K>(), // [verus]
                old(self).wf(), // [verus]
                self.slots@ == old(self).slots@, // [verus]
                self.len == old(self).len, // [verus]
                slots.len() == cap, // [verus]
                is_pow2(cap), // [verus]
                self.slots.len() < cap, // [verus]
                iter.index@ <= self.slots.len(), // [verus]
                occ(slots@).len() <= occ_below(self.slots@, iter.index@ as int).len(), // [verus]
                clustered(slots@), // [verus]
                unique(slots@), // [verus]
                view(slots@) == view_below(self.slots@, iter.index@ as int), // [verus]
        {
            let ghost j = iter.index@; // [verus]
            assert((k, v) == self.slots@[j as int]); // [verus]
            proof { lemma_view_below_step(self.slots@, j as int); lemma_view_dom(slots@, 0); lemma_view_dom(self.slots@, j as int); } // [verus]
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
                proof { // [verus]
                    assert(!view_below(self.slots@, j as int).contains_key(k)); // [verus]
                    assert(!has_key(slots@, k)); // [verus]
                    lemma_probe_finds(slots@, k, i as int); // [verus]
                    lemma_occ_write(slots@, i as int, k, v); // [verus]
                    lemma_insert_empty(slots@, k, v, i as int); // [verus]
                } // [verus]
                // [canary]
                slots[i] = (k, v);
            }
        }
        proof { // [verus]
            let n = self.slots.len() as int; // [verus]
            lemma_occ_finite(self.slots@, n); // [verus]
            assert(occ_below(self.slots@, n) =~= occ(self.slots@)); // [verus]
            lemma_view_below_ends(self.slots@); // [verus]
        } // [verus]
        // [canary]
        self.slots = slots;
        Ok(())
    }
}

} // verus!

fn main() {}
