// The canaries `rake verus` adds to swar.rs: each `canary_` function calls the
// proved code from outside, then asserts `false` - which must FAIL. One that
// verifies means the specs above prove anything there.

fn canary_word(s: &[u8]) {
    if s.len() >= 8 {
        let _w = word(s);
        assert(false);
    }
}

fn canary_has_zero(v: u64) {
    let _m = has_zero(v);
    assert(false);
}

fn canary_seek4(input: &[u8], data: usize, end: usize, c: [u8; 4]) {
    let _r = seek4(input, data, end, c);
    assert(false);
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_seek4() {
    // "aaaaaaaaaa<aaaaa": the '<' at 10.
    let a: [u8; 16] = [
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61,
        0x61, 0x61, 0x3c, 0x61, 0x61, 0x61, 0x61, 0x61,
    ];
    let c: [u8; 4] = [0x3c, 0x26, 0x0d, 0x00];
    let r = seek4(&a, 3, 16, c);
    // Never before where it started, never past the '<'.
    assert(r >= 3);
    assert(r <= 10) by {
        if r > 10 {
            assert(wanted(a@[10], c));
        }
    }
}
