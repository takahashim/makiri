// The canaries `rake verus` adds to int.rs: each `canary_` function reaches
// one outcome of the proved code from outside, at each width, then asserts
// `false` - which must FAIL. One that verifies means the specs above prove
// anything there.

fn canary_fixable_32(n: i64) {
    if long32::fixable_in::<i32>(n).is_some() {
        assert(false);
    }
}

fn canary_unfixable_32(n: i64) {
    if long32::fixable_in::<i32>(n).is_none() {
        assert(false);
    }
}

fn canary_fixable_64(n: i64) {
    if long64::fixable_in::<i64>(n).is_some() {
        assert(false);
    }
}

fn canary_unfixable_64(n: i64) {
    if long64::fixable_in::<i64>(n).is_none() {
        assert(false);
    }
}

fn canary_posfixable(n: u64) {
    if long32::posfixable_in::<i32>(n).is_some() {
        assert(false);
    }
}

fn canary_bits(l: i32) {
    let _v = long32::fixnum_bits(l);
    assert(false);
}

fn canary_len(n: i64) {
    let _r = long64::len(n);
    assert(false);
}

fn canary_capacity(n: usize) {
    let _r = long32::capacity(n);
    assert(false);
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_int() {
    // A Windows `long`: 2^30 - 1 is the largest Fixnum.
    let a = long32::fixable_in::<i32>(0x3fff_ffff);
    assert(a.is_some());
    let a = long32::fixable_in::<i32>(0x4000_0000);
    assert(a.is_none());
    let a = long32::fixable_in::<i32>(-0x4000_0000);
    assert(a.is_some());
    let a = long32::fixable_in::<i32>(-0x4000_0001);
    assert(a.is_none());
    // 2^62 is no 64-bit Fixnum either, and 2^62 - 1 is one.
    let b = long64::fixable_in::<i64>(0x4000_0000_0000_0000);
    assert(b.is_none());
    let b = long64::posfixable_in::<i64>(0x3fff_ffff_ffff_ffff);
    assert(b.is_some());
    let b = long64::posfixable_in::<i64>(u64::MAX);
    assert(b.is_none());
    // -1's Fixnum has the flag set.
    let v = long64::fixnum_bits(-1);
    assert(v & 1 == 1);
    // A length is never negative; a capacity never past LONG_MAX.
    let r = long32::len(-5);
    assert(r == 0);
    let c = long32::capacity(usize::MAX);
    assert(c == i32::MAX);
    let c = long64::capacity(7);
    assert(c == 7);
}

/// A 32-bit Fixnum is a 64-bit one: code tried on Linux or macOS sees every
/// value a Windows build would take, and more.
fn narrower_is_inside(n: u64) {
    let a = long32::posfixable_in::<i32>(n);
    let b = long64::posfixable_in::<i64>(n);
    assert(a.is_some() ==> b.is_some());
}
