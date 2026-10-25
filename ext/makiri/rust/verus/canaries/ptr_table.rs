// The canaries `rake verus` adds to ptr_table.rs: each `canary_` function
// reaches one outcome of the proved code, from tables it builds itself, then
// asserts `false` - which must FAIL. One that verifies means the axioms, or
// the specs above, prove anything there: the proof is vacuous.

impl TableKey for u64 {
    const EMPTY: u64 = 0;
    open spec fn hash_spec(self) -> u64 {
        self
    }
    fn table_hash(self) -> u64 {
        self
    }
}

proof fn u64_eq_is_equality()
    ensures
        eq_is_equality::<u64>(),
{
}

fn empty_map() -> (m: PtrMap<u64, u64>)
    ensures
        m.wf(),
        m.map() == Map::<u64, u64>::empty(),
{
    let m = PtrMap { slots: Vec::new(), len: 0 };
    assert(occ(m.slots@) =~= Set::<int>::empty());
    proof { lemma_all_empty(m.slots@); }
    m
}

fn canary_capacity_for(keys: usize) {
    if capacity_for(keys).is_some() {
        assert(false);
    }
}

fn canary_table_insert() {
    proof { u64_eq_is_equality(); }
    if let Some(mut t) = PtrTable::<u64, u64>::with_keys(4, 0) {
        if t.insert(3, 7).is_some() {
            assert(false);
        }
    }
}

fn canary_table_get() {
    proof { u64_eq_is_equality(); }
    if let Some(mut t) = PtrTable::<u64, u64>::with_keys(4, 0) {
        let _ = t.insert(3, 7);
        if t.get(3).is_some() {
            assert(false);
        }
    }
}

fn canary_map_insert() {
    proof { u64_eq_is_equality(); }
    let mut m = empty_map();
    if m.insert(5, 1).is_ok() {
        assert(false);
    }
}

fn canary_map_set_then_get() {
    proof { u64_eq_is_equality(); }
    let mut m = empty_map();
    let _ = m.insert(5, 1);
    if m.set(5, 2).is_ok() {
        let _ = m.get(5);
        assert(false);
    }
}

fn canary_map_clear() {
    proof { u64_eq_is_equality(); }
    let mut m = empty_map();
    let _ = m.insert(5, 1);
    m.clear();
    if m.insert(6, 1).is_ok() {
        assert(false);
    }
}

// Not canaries: what the map specs promise, used as a caller would. These
// must verify - an error here is a spec too weak to use.

fn uses_map_insert_get_set_clear() {
    proof { u64_eq_is_equality(); }
    let mut m = empty_map();
    if m.insert(5, 1).is_ok() {
        let a = m.get(5);
        let b = m.get(6);
        assert(a == Some(1u64) && b.is_none());
        if m.insert(5, 9).is_ok() {
            let c = m.get(5);
            assert(c == Some(1u64));
        }
        if m.set(5, 2).is_ok() {
            let d = m.get(5);
            assert(d == Some(2u64));
        }
    }
    m.clear();
    let e = m.get(5);
    assert(e.is_none());
}

fn uses_table_insert_get() {
    proof { u64_eq_is_equality(); }
    if let Some(mut t) = PtrTable::<u64, u64>::with_keys(4, 0) {
        let a = t.get(3);
        assert(a.is_none());
        if t.insert(3, 7).is_some() {
            let b = t.get(3);
            assert(b == Some(7u64));
        }
    }
}
