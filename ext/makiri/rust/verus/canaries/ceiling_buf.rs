// The canaries `rake verus` adds to ceiling_buf.rs: each `canary_` function
// reaches one outcome of the proved code, from a buffer it builds itself,
// then asserts `false` - which must FAIL. One that verifies means the
// axioms, or the specs above, prove anything there: the proof is vacuous.

fn small() -> (b: Buf)
    ensures
        b.wf(),
        b.view() == Seq::<u8>::empty(),
        limit_for(b.max) == 8,
{
    let b = Buf { data: Vec::new(), len: 0, max: 8 };
    proof { lemma_limits(); }
    assert(b.view() =~= Seq::<u8>::empty());
    b
}

fn canary_append_ok() {
    let mut b = small();
    if b.append(&[1, 2, 3]).is_ok() {
        let _ = b.as_slice();
        assert(false);
    }
}

fn canary_append_limit() {
    let mut b = small();
    if b.append(&[0; 9]) == Err::<(), BufError>(BufError::Limit) {
        assert(false);
    }
}

fn canary_append_oom() {
    let mut b = small();
    if b.append(&[1, 2, 3]) == Err::<(), BufError>(BufError::Oom) {
        assert(false);
    }
}

fn canary_reserve_then_append() {
    let mut b = small();
    let _ = b.reserve(4);
    if b.append(&[1, 2]).is_ok() {
        assert(false);
    }
}

fn canary_raise_limit_then_steal() {
    let mut b = small();
    b.raise_limit(16);
    let _ = b.append(&[1, 2]);
    let _ = b.steal();
    assert(false);
}

fn canary_copy_small(dst: &mut [u8], src: &[u8])
    requires
        old(dst)@.len() == src@.len(),
{
    copy_small(dst, src);
    assert(false);
}
