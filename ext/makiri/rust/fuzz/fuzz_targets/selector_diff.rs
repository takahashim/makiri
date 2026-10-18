//! The ported selector parser (`makiri::selectors`) against Lexbor's, over
//! arbitrary text: both parse the same selector and must build the same
//! lists (`makiri::lexbor::selector_diff`). `docs/rewrite_parser.ja.md` stage
//! 5's oracle. Input that is not what Makiri hands a parser - invalid UTF-8,
//! or a NUL - is skipped.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::lexbor::selector_diff::check;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = core::str::from_utf8(data) else {
        return;
    };
    if s.contains('\0') {
        return;
    }
    check(s);
});
