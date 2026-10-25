// The canaries `rake verus` adds to limits.rs: each `canary_` function
// reaches one outcome of `admit`, then asserts `false` - which must FAIL. One
// that verifies means the specs above prove anything there.

fn canary_admitted(count: usize, n: usize, cap: usize) {
    if admit(count, n, cap).is_some() {
        assert(false);
    }
}

fn canary_refused(count: usize, n: usize, cap: usize) {
    if admit(count, n, cap).is_none() {
        assert(false);
    }
}

proof fn canary_run(ns: Seq<int>) {
    run_stays_within(0, ns, 10);
    assert(false);
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_admit_up_to_the_cap() {
    let a = admit(0, 3, 5);
    assert(a == Some(3usize));
    let b = admit(3, 2, 5);
    assert(b == Some(5usize));
    let c = admit(5, 1, 5);
    assert(c.is_none());
}
