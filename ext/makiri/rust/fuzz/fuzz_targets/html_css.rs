//! CSS selectors over a Lexbor-parsed HTML document, through the HTML matcher.
//!
//! `css` covers `Makiri::XML`'s CSS (the lowering to XPath) and `html` the HTML
//! parse. This is the pair `Node#css` / `#at_css` / `#matches?` run: Lexbor's
//! selector parser behind `contains_guard` (`css_parser::parse`), then
//! `lexbor::css_match` - `compile`, the chain loop and its backtracking,
//! the task stack for `:is()` / `:not()` / `:has()` / `of S`, the sibling-
//! position memo, the lazily resolved names and the adapter reads they make -
//! over an arbitrary document, with one `Scratch` reused across the calls as
//! the selector engine reuses its own.
//!
//! The bytes up to the first NUL are the selector (a selector cannot hold one:
//! the bridge refuses it, as `VerifiedText` does), and the rest is the document
//! (arbitrary bytes are valid HTML input).
//!
//! Beyond memory safety, one invariant the matcher owes: the walking query and
//! the one-candidate query agree. They reach the same verdict by different
//! routes - names resolved to Lexbor ids or compared as bytes, sibling
//! positions remembered or recounted, the rightmost compound's tag filter or
//! none - so an element is in `select_all` exactly when `matches_any` says it
//! matches, and `select_first` is the first of `select_all`. A query that
//! refused (a budget, an unsupported construct) is not compared.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::gvl::Gvl;
use makiri::lexbor::adapter::html::HtmlNode;
use makiri::lexbor::adapter::post_parse::parse_html;
use makiri::lexbor::adapter::tree_guard::DepthLimit;
use makiri::lexbor::css_match::{matches_any, select_all, select_first, Scratch};
use makiri::lexbor::css_parser;

/// How many elements are asked `matches?` each: each is a full query, with
/// its own budget, so this bounds the run, not the check.
const ONE_BY_ONE: usize = 64;

fuzz_target!(|data: &[u8]| {
    let Some(sep) = data.iter().position(|&b| b == 0) else {
        return;
    };
    let (selector, html) = (&data[..sep], &data[sep + 1..]);
    let Some(expr) = Expr::new(selector) else {
        return;
    };
    let Some(text) = expr.text() else {
        return;
    };
    let Ok(p) = parse_html(html, false, DepthLimit::DEFAULT) else {
        return;
    };
    let gvl = Gvl::exclusive();
    let Ok(parsed) = css_parser::parse(&gvl, text) else {
        return;
    };
    let groups = parsed.groups();
    // SAFETY: `p` owns the document and outlives every handle made here.
    let root = unsafe { p.raw_doc().as_doc() }.as_node();
    let mut scratch = Scratch::new();

    let all = select_all(&mut scratch, root, groups);
    let first = select_first(&mut scratch, root, groups);
    let Ok(all) = all else {
        return;
    };
    if let Ok(first) = first {
        assert!(
            first == all.first().copied(),
            "select_first is not the first of select_all"
        );
    }
    for el in root
        .subtree()
        .filter_map(HtmlNode::element)
        .take(ONE_BY_ONE)
    {
        let Ok(matched) = matches_any(&mut scratch, groups, el) else {
            return;
        };
        assert_eq!(
            matched,
            all.contains(&el.node()),
            "matches? and css disagree on an element"
        );
    }
});
