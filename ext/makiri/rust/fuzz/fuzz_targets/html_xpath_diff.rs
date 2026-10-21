//! XPath's HTML instance over the arena against the one over Lexbor's DOM,
//! over an arbitrary document (`makiri::lexbor::xpath_diff`).
//!
//! `html_xpath` holds the arena's instance to its own promises; the engine
//! is shared, so a wrong answer from the arena's `Dom` - a name test's case
//! folding, strict or lax namespace matching, the attribute axis, document
//! order, a string-value, the `//tag` index path - passes there. Here the
//! same Lexbor parse is read through both `Dom`s, the arena through its
//! copy, and the answers are compared by walk position.
//!
//! The first byte picks the context node (the document or an element); the
//! bytes after it, up to the first NUL, choose the expression's shape from
//! `xpath_diff::expression`'s grammar (an arbitrary string rarely parses);
//! the rest is the document.
#![no_main]

use libfuzzer_sys::fuzz_target;
use makiri::lexbor::xpath_diff::{check, expression};

fuzz_target!(|data: &[u8]| {
    let Some((&at, rest)) = data.split_first() else {
        return;
    };
    let sep = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    let (choices, html) = (&rest[..sep], rest.get(sep + 1..).unwrap_or_default());
    /* Past the last choice byte every pick is 0: the shortest form. */
    let mut next = choices.iter();
    let expr = expression(&mut |n| next.next().map_or(0, |&b| usize::from(b) % n.max(1)));
    if let Err(diff) = check(html, &expr, usize::from(at)) {
        panic!("the arena's XPath differs from Lexbor's\n{diff}");
    }
});
