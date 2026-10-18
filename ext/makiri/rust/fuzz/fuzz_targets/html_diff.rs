//! The ported tree builder (`makiri::html::tree`) against Lexbor's, over
//! arbitrary bytes: both parse the same input - as a document, or as a
//! fragment in one of a set of contexts - and their trees, copied into the
//! arena for Lexbor, must dump the same (`makiri::lexbor::tree_diff`), source
//! offsets included. `docs/rewrite_parser.ja.md` stage 4's oracle.
//!
//! The first byte picks the mode: 0 a document, otherwise a fragment context
//! from [`CONTEXTS`]; the rest is the input. No guard limit is set, so a
//! difference is never a refusal on one side only.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::lexbor::tree_diff::{check, check_fragment};

const CONTEXTS: &[&str] = &[
    "body", "div", "td", "tr", "table", "tbody", "caption", "colgroup", "select", "template",
    "title", "textarea", "style", "script", "noscript", "plaintext", "html", "head", "form",
    "frameset", "svg svg", "svg desc", "svg foreignObject", "math math", "math mi",
    "math annotation-xml", "option", "optgroup", "p", "li", "xmp", "iframe",
];

fuzz_target!(|data: &[u8]| {
    let Some((&mode, input)) = data.split_first() else {
        return;
    };
    let result = match mode as usize % (CONTEXTS.len() + 1) {
        0 => check(input, true),
        i => check_fragment(input, CONTEXTS[i - 1]),
    };
    if let Err(diff) = result {
        panic!("the port's tree differs from Lexbor's\n{diff}");
    }
});
