// The canaries `rake verus` adds to source_lines.rs: each `canary_` function
// calls the proved code from outside, then asserts `false` - which must FAIL.
// One that verifies means the specs above prove anything there.

fn canary_built(bytes: &[u8]) {
    if lines_build(bytes).is_some() {
        assert(false);
    }
}

fn canary_refused(bytes: &[u8]) {
    if lines_build(bytes).is_none() {
        assert(false);
    }
}

fn canary_mask(w: u64) {
    let _m = newline_mask(w);
    assert(false);
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_lines_build() {
    // "a\nb": line 2 starts at 2.
    let a: [u8; 3] = [0x61, 0x0a, 0x62];
    if let Some(lines) = lines_build(&a) {
        assert(a@.len() == 3 && a@[0] == 0x61 && a@[1] == 0x0a && a@[2] == 0x62);
        assert(starts_of(a@, 0) == seq![0usize]);
        assert(starts_of(a@, 1) == seq![0usize]);
        assert(starts_of(a@, 2) =~= seq![0usize, 2]);
        assert(starts_of(a@, 3) == starts_of(a@, 2));
        assert(lines.starts@ == seq![0usize, 2]);
    }
}
