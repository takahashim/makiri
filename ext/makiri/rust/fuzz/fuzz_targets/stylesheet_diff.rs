//! The ported stylesheet parser (`makiri::selectors::parse_stylesheet`)
//! against Lexbor's, over arbitrary text: both parse the same stylesheet and
//! must build the same rules (`makiri::lexbor::stylesheet_diff`). Input that
//! is not what Makiri hands a parser - invalid UTF-8, or a NUL - is skipped.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::lexbor::stylesheet_diff::check;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = core::str::from_utf8(data) else {
        return;
    };
    if s.contains('\0') {
        return;
    }
    check(s.as_bytes());
});
