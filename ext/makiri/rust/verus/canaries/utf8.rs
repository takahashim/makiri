// The canaries `rake verus` adds to utf8.rs: each `canary_` function reaches
// one outcome of the decoder or the scan, then asserts `false` - which must
// FAIL. One that verifies means the specs above prove anything there.

fn canary_decoded(p: &[u8]) {
    if decode1(p).is_some() {
        assert(false);
    }
}

fn canary_refused(p: &[u8]) {
    if decode1(p).is_none() {
        assert(false);
    }
}

fn canary_accepted(s: &[u8]) {
    if validate_chars(s) {
        assert(false);
    }
}

fn canary_rejected(s: &[u8]) {
    if !validate_chars(s) {
        assert(false);
    }
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_decode1() {
    let a: [u8; 1] = [0x41];
    let r = decode1(&a);
    proof { ascii(a@[0]); }
    assert(r == Some((0x41u32, 1usize)));
    // An overlong form: 0xC0 0x80 would be U+0000 in two bytes.
    let c: [u8; 2] = [0xC0, 0x80];
    let r = decode1(&c);
    assert(codepoint_width_2(0xC0u8, 0x80u8) == 0) by (compute_only);
    assert(r.is_none());
}
