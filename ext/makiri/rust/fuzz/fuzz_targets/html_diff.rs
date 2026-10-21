//! The ported tree builder (`makiri::html::tree`) against Lexbor's, over
//! arbitrary bytes: both parse the same input - as a document, or as a
//! fragment in one of a set of contexts - and their trees, copied into the
//! arena for Lexbor, must dump the same (`makiri::lexbor::tree_diff`), source
//! offsets included. `docs/rewrite_parser.ja.md` stage 4's oracle. Where
//! the trees agree, so must their serializations, every form
//! (`check_serialization`).
//!
//! The first byte picks the mode: 0 a document, otherwise a fragment context
//! from [`CONTEXTS`]; the rest is the input. No guard limit is set for the
//! tree comparison, so a difference there is never a refusal on one side
//! only; the second byte then picks a small depth limit, under which both
//! parsers must accept or refuse alike (`check_verdict`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::lexbor::tree_diff::{
    check, check_fragment, check_fragment_serialization, check_fragment_verdict,
    check_serialization, check_verdict,
};
use makiri::lexbor::adapter::tree_guard::DepthLimit;

const CONTEXTS: &[&str] = &[
    "body",
    "div",
    "td",
    "tr",
    "table",
    "tbody",
    "caption",
    "colgroup",
    "select",
    "template",
    "title",
    "textarea",
    "style",
    "script",
    "noscript",
    "plaintext",
    "html",
    "head",
    "form",
    "frameset",
    "svg svg",
    "svg desc",
    "svg foreignObject",
    "math math",
    "math mi",
    "math annotation-xml",
    "option",
    "optgroup",
    "p",
    "li",
    "xmp",
    "iframe",
];

fuzz_target!(|data: &[u8]| {
    let Some((&mode, input)) = data.split_first() else {
        return;
    };
    /* Depths 1..=32: deep enough for the guard's interesting cases (foster
     * parenting, templates, foreign content, the adoption agency's clones)
     * within an input libFuzzer grows to. */
    let limit = DepthLimit::at_most(usize::from(input.first().copied().unwrap_or(0) % 32) + 1);
    let result = match mode as usize % (CONTEXTS.len() + 1) {
        0 => check(input, true)
            .and_then(|()| check_serialization(input))
            .and_then(|()| check_verdict(input, limit)),
        i => {
            let context = CONTEXTS[i - 1];
            check_fragment(input, context)
                .and_then(|()| check_fragment_serialization(input, context))
                .and_then(|()| check_fragment_verdict(input, context, limit))
        }
    };
    if let Err(diff) = result {
        panic!("the port differs from Lexbor\n{diff}");
    }
});
