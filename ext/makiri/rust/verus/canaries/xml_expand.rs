// The canaries `rake verus` adds to xml_expand.rs: each `canary_` function
// reaches one outcome of the proved code from outside, then asserts `false` -
// which must FAIL. One that verifies means the specs above prove anything
// there.

fn canary_char_ref_ok(s: &[u8]) {
    if scan_char_ref(s).is_ok() {
        assert(false);
    }
}

fn canary_char_ref_err(s: &[u8]) {
    if scan_char_ref(s).is_err() {
        assert(false);
    }
}

fn canary_expanded(src: &[u8], out: &mut [u8]) {
    if out.len() >= src.len() {
        if expand_into(src, ExpandMode::Text, out).is_ok() {
            assert(false);
        }
    }
}

fn canary_refused(src: &[u8], out: &mut [u8]) {
    if out.len() >= src.len() {
        if expand_into(src, ExpandMode::Attr, out).is_err() {
            assert(false);
        }
    }
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_scan_char_ref() {
    // "65;": 'A'.
    let a: [u8; 3] = [0x36, 0x35, 0x3b];
    let r = scan_char_ref(&a);
    assert(start(a@) == 0 && !hexp(a@));
    assert(is_digit(a@, 0) && is_digit(a@, 1) && !is_digit(a@, 2));
    assert(run_end(a@, 2) == 2);
    assert(run_end(a@, 1) == 2);
    assert(run_end(a@, 0) == 2);
    assert(value(a@, 0) == 0);
    assert(value(a@, 1) == 6);
    assert(value(a@, 2) == 65);
    assert(r == Ok::<(u32, usize), ExpandErr>((65u32, 3usize)));
    // "x110000;": one past U+10FFFF - refused, not wrapped.
    let b: [u8; 8] = [0x78, 0x31, 0x31, 0x30, 0x30, 0x30, 0x30, 0x3b];
    let r = scan_char_ref(&b);
    assert(hexp(b@) && start(b@) == 1);
    assert(run_end(b@, 7) == 7);
    assert(run_end(b@, 6) == 7);
    assert(run_end(b@, 5) == 7);
    assert(run_end(b@, 4) == 7);
    assert(run_end(b@, 3) == 7);
    assert(run_end(b@, 2) == 7);
    assert(run_end(b@, 1) == 7);
    assert(value(b@, 1) == 0);
    assert(value(b@, 2) == 1);
    assert(value(b@, 3) == 0x11);
    assert(value(b@, 4) == 0x110);
    assert(value(b@, 5) == 0x1100);
    assert(value(b@, 6) == 0x11000);
    assert(value(b@, 7) == 0x110000);
    assert(r.is_err());
}
