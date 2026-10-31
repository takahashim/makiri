// The canaries `rake verus` adds to xpath_number.rs: each `canary_` function
// reaches one outcome of `extent`, then asserts `false` - which must FAIL. One
// that verifies means the specs above prove anything there.

fn canary_number(s: &[u8]) {
    if extent(s) > 0 {
        assert(false);
    }
}

fn canary_none(s: &[u8]) {
    if extent(s) == 0 {
        assert(false);
    }
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_extent() {
    let a: [u8; 3] = [0x31, 0x2e, 0x78]; // "1.x"
    let n = extent(&a);
    // "1." is a Number (the '.' at 1); "1.x" is not, 'x' being no digit.
    assert(is_number(a@, 2)) by {
        assert(0 <= 1 < 2 && a@[1] == 0x2e && digits(a@, 0, 1) && digits(a@, 2, 2));
    }
    assert(!is_number(a@, 3)) by {
        assert(!is_digit(a@[2]));
        assert(!is_digit(a@[1]));
    }
    assert(n == 2);
    let b: [u8; 1] = [0x2e]; // "."
    let m = extent(&b);
    assert(m == 0);
}
