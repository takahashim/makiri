//! CSS selectors over a parsed HTML document, through the HTML matcher.
//!
//! `css` covers `Makiri::XML`'s CSS (the same matcher over the XML arena) and
//! `html` the HTML parse. This is the path `Node#css` / `#at_css` / `#matches?` run: the
//! compiled-selector cache (`css_cache::with_compiled` - the parser
//! port, the cached lists, the flush at the cap,
//! the bypass and its re-test, and the engine's kept `Scratch`), then
//! `css_match` - `compile`, the chain loop and its backtracking,
//! the task stack for `:is()` / `:not()` / `:has()` / `of S`, the sibling-
//! position memo, the lazily resolved names and the adapter reads they make -
//! over an arbitrary document.
//!
//! Each input queries its selector twice, as a repeated `Node#css` would: a
//! miss (or a bypassed parse) and then, while caching is on, a hit. The cache
//! is process-global, so its state - and which of its paths an input takes -
//! carries from one input to the next, as it does in a Ruby process.
//!
//! The bytes up to the first NUL are the selector (a selector cannot hold one:
//! the bridge refuses it, as `VerifiedText` does), and the rest is the document
//! (arbitrary bytes are valid HTML input).
//!
//! Beyond memory safety, one invariant the matcher owes: the walking query and
//! the one-candidate query agree. They reach the same verdict by different
//! routes - names resolved to ids or compared as bytes, sibling
//! positions remembered or recounted, the rightmost compound's tag filter or
//! none - so an element is in `select_all` exactly when `matches_any` says it
//! matches, and `select_first` is the first of `select_all`. A query that
//! refused (a budget, an unsupported construct) is not compared.
#![no_main]
#![allow(clippy::disallowed_methods)] /* the harness, not the engine: see common.rs */

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::css_cache;
use makiri::css_match::{matches_any, select_all, select_first, Namespaces, Scratch};
use makiri::html::dom::css::HtmlHost;
use makiri::html_dom::html::HtmlNode;
use makiri::html_dom::post_parse::parse_html;
use makiri::html_dom::tree_guard::ParseLimits;
use makiri::selectors::ast::{FunctionArg, Lists, PseudoClass, Simple};

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
    let Ok(p) = parse_html(html, false, ParseLimits::DEFAULT) else {
        return;
    };
    let root = p.doc().as_node();
    for _ in 0..2 {
        let answered = css_cache::with_compiled(text.as_bytes(), |groups, scratch| {
            check(root, groups, scratch)
        });
        // A refusal is the budget doing its job, and each refused query spends
        // the whole of it: asking again (or the same query a second way) only
        // multiplies the run, which under ASan went past libFuzzer's timeout.
        if !matches!(answered, Ok(true)) {
            return;
        }
    }
});

/// The walking query and the one-candidate query agree (module doc). False
/// when a query refused, so the caller stops.
fn check(root: HtmlNode<'_>, groups: Lists<'_>, scratch: &mut Scratch<HtmlHost>) -> bool {
    if mentions_scope(groups) {
        return false;
    }
    let Ok(all) = select_all(scratch, root, groups, &Namespaces::NONE) else {
        return false;
    };
    if let Ok(first) = select_first(scratch, root, groups, &Namespaces::NONE) {
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
        let Ok(matched) = matches_any(scratch, groups, &Namespaces::NONE, el.node()) else {
            return false;
        };
        assert_eq!(
            matched,
            all.contains(&el.node()),
            "matches? and css disagree on an element"
        );
    }
    true
}

/// Whether the selector holds `:scope`, nested or not. Walked on a work list
/// (no input nests the harness's stack); one too large to walk counts as
/// holding it.
fn mentions_scope(groups: Lists<'_>) -> bool {
    let mut pending = vec![groups];
    let mut visited = 0usize;
    while let Some(lists) = pending.pop() {
        for list in lists {
            let mut sel = list.first();
            while let Some(s) = sel {
                visited += 1;
                if visited > 4096 {
                    return true;
                }
                match s.simple() {
                    Simple::PseudoClass(PseudoClass::Scope) => return true,
                    Simple::PseudoClassFunction(FunctionArg::Selectors { lists, .. }) => {
                        pending.push(lists)
                    }
                    Simple::PseudoClassFunction(FunctionArg::Nth { anb: Some(anb), .. }) => {
                        if let Some(of) = anb.of {
                            pending.push(of);
                        }
                    }
                    _ => {}
                }
                sel = s.next();
            }
        }
    }
    false
}
