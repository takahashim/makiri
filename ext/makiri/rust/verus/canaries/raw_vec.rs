// The canaries `rake verus` adds to raw_vec.rs: each `canary_` function
// reaches one outcome of the proved code under its stated precondition, then
// asserts `false` - which must FAIL. One that verifies means the axioms, or
// the specs above, prove anything there: the proof is vacuous.

fn canary_grow_capacity_some(cap: usize, need: usize, elem: usize) {
    if falloc::grow_capacity(cap, need, elem).is_some() {
        assert(false);
    }
}

fn canary_grow_capacity_none(cap: usize, need: usize, elem: usize) {
    if falloc::grow_capacity(cap, need, elem).is_none() {
        assert(false);
    }
}

fn canary_push_ok(v: &mut RawVec<u8>, x: u8)
    requires
        old(v).wf(),
{
    if v.push(x).is_ok() {
        let _ = v.as_slice();
        assert(false);
    }
}

fn canary_push_err(v: &mut RawVec<u8>, x: u8)
    requires
        old(v).wf(),
{
    if v.push(x).is_err() {
        assert(false);
    }
}

fn canary_reserve_ok(v: &mut RawVec<u8>, n: usize)
    requires
        old(v).wf(),
{
    if v.reserve(n).is_ok() {
        assert(false);
    }
}
