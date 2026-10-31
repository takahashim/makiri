// The canaries `rake verus` adds to xml_chars.rs: each `canary_` function
// reaches one answer of a class, then asserts `false` - which must FAIL. One
// that verifies means the specs above prove anything there.

fn canary_name_start(c: u32) {
    if is_name_start(c) {
        assert(false);
    }
}

fn canary_not_name_char(c: u32) {
    if !is_name_char(c) {
        assert(false);
    }
}

fn canary_char(c: u32) {
    if is_char(c) {
        assert(false);
    }
}

fn canary_not_char(c: u32) {
    if !is_char(c) {
        assert(false);
    }
}

// Not canaries: the specs used as a caller would. These must verify.

fn uses_the_classes() {
    let a = is_name_start(0x41); // 'A'
    assert(a);
    let digit_starts = is_name_start(0x30); // '0' cannot start a name
    assert(!digit_starts);
    let digit_continues = is_name_char(0x30); // but can continue one
    assert(digit_continues);
    let fffe = is_char(0xFFFE); // not a Char
    assert(!fffe);
    let surrogate = is_char(0xD800);
    assert(!surrogate);
}
