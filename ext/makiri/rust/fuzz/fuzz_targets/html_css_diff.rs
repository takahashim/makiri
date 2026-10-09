//! The HTML CSS matcher against the engine it replaced, over an arbitrary
//! document.
//!
//! `html_css` holds `css_match` to its own promises - memory safety, and the
//! walking and one-candidate queries agreeing - but its three queries share
//! `compile`, `step_chain` and `check_simple`, so a mistake in any of those is
//! made by all three and passes. This target compares `select_all` with what
//! the OLD engine, Lexbor's own `lxb_selectors` (`lexbor::selectors`, built
//! here through the `css-reference` feature), answers for the same selector
//! over the same document: the coverage-guided counterpart of
//! `lexbor::tests::css_match`'s randomized differential tests. What it is
//! after is a silent wrong answer, above all from the shortcuts Lexbor does
//! not take - `step_chain`'s `Fail` pruning, the inline `:is()` / `:not()`,
//! the tag filter, names resolved to ids, the sibling-position memo.
//!
//! The input is laid out as in `html_css`: the selector up to the first NUL,
//! then the document.
//!
//! Only what the two engines are meant to agree on is compared. `css_match`'s
//! module doc lists where it departs from Lexbor on purpose, and a selector
//! reaching one of those is skipped ([`comparable`]): form state
//! (`:checked` / `:disabled` / `:enabled`, and `:read-only` / `:read-write`,
//! which ask the same `is_disabled`), `of S`, `An+B` past 2^53, a
//! compound that starts with a list pseudo-class where it has several
//! candidates (Lexbor tries only the first - found by this target), and,
//! on a document with foreign elements, the HTML Standard's case-sensitivity
//! and qualified-name rules for type and attribute selectors. A selector either engine refuses is not compared either.
//!
//! The old engine backtracks exhaustively, which is exponential in a chain's
//! descendant and subsequent-sibling combinators; [`affordable`] skips the
//! inputs where that would turn a run into a timeout rather than a finding.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::gvl::Gvl;
use makiri::lexbor::adapter::html::{HtmlNode, RawNode};
use makiri::lexbor::adapter::post_parse::parse_html;
use makiri::lexbor::adapter::tree_guard::DepthLimit;
use makiri::lexbor::css_match::{select_all, Scratch};
use makiri::lexbor::css_parser::{
    Combinator, FunctionArg, ListPseudo, Lists, PseudoClass, Selector, Simple,
};
use makiri::lexbor::selector_cache;
use makiri::lexbor::selectors as old_engine;

const XHTML: &[u8] = b"http://www.w3.org/1999/xhtml";

/// The old engine's work is bounded by roughly `elements ^ (k + 1)`, `k` the
/// backtracking points ([`Shape::backtracks`]); above this the input is
/// skipped.
const OLD_ENGINE_WORK: f64 = 2.0e7;

/// An `An+B` coefficient past this is computed exactly here and in `double`
/// by Lexbor (`css_match`'s module doc).
const ANB_EXACT: core::ffi::c_ulong = 1 << 53;

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
    // SAFETY: `p` owns the document and outlives every handle made here.
    let root = unsafe { p.raw_doc().as_doc() }.as_node();
    let doc = Doc::survey(root);

    let new = selector_cache::with_compiled(&gvl, text.as_bytes(), |groups, scratch| {
        new_answer(root, groups, scratch, &doc)
    });
    let Ok(Some(new)) = new else {
        return;
    };
    let Ok(old) = old_engine::select_all(&gvl, RawNode::from(root), text.as_bytes()) else {
        return;
    };
    assert!(
        new == old,
        "css_match and Lexbor's engine disagree: {} match(es) here, {} there",
        new.len(),
        old.len()
    );
});

/// `css_match`'s answer, or None when the selector is not to be compared.
fn new_answer(
    root: HtmlNode<'_>,
    groups: Lists<'_>,
    scratch: &mut Scratch,
    doc: &Doc,
) -> Option<Vec<RawNode>> {
    let shape = Shape::of(groups)?;
    if !comparable(&shape, doc) || !affordable(&shape, doc) {
        return None;
    }
    let all = select_all(scratch, root, groups).ok()?;
    Some(all.into_iter().map(RawNode::from).collect())
}

/// What the document holds that decides whether a selector can be compared.
struct Doc {
    elements: usize,
    /// An element outside the HTML namespace (SVG, MathML).
    foreign: bool,
    /// A foreign element with an upper-case letter in its name, or with an
    /// attribute whose name has one or is prefixed (`viewBox`,
    /// `xlink:href`): where the two engines name things differently.
    foreign_names: bool,
}

impl Doc {
    fn survey(root: HtmlNode<'_>) -> Doc {
        let mut doc = Doc {
            elements: 0,
            foreign: false,
            foreign_names: false,
        };
        for el in root.subtree().filter_map(HtmlNode::element) {
            doc.elements += 1;
            if el.node().ns_uri() == Some(XHTML) {
                continue;
            }
            doc.foreign = true;
            let upper = |name: &[u8]| name.iter().any(u8::is_ascii_uppercase);
            if upper(el.qualified_name())
                || el
                    .attrs()
                    .any(|a| upper(a.qualified_name()) || a.qualified_name().contains(&b':'))
            {
                doc.foreign_names = true;
            }
        }
        doc
    }
}

/// What the selector holds, over every list it nests.
#[derive(Default)]
struct Shape {
    /// A construct `css_match` answers on purpose differently from Lexbor,
    /// or one it refuses.
    departs: bool,
    /// A type or attribute selector.
    names: bool,
    /// A type or attribute selector whose name has an upper-case letter.
    upper_names: bool,
    /// Descendant / subsequent-sibling combinators and `:has()` arguments:
    /// the places Lexbor's matcher backtracks from.
    backtracks: u32,
}

impl Shape {
    /// Walks the selector tree on a work list. None when it is deeper or
    /// wider than any comparison the old engine could afford.
    fn of(groups: Lists<'_>) -> Option<Shape> {
        let mut shape = Shape::default();
        // Each list with whether it is inside a `:has()` argument.
        let mut work: Vec<(Lists<'_>, bool)> = vec![(groups, false)];
        let mut visited = 0usize;
        while let Some((lists, in_has)) = work.pop() {
            for list in lists {
                // Whether the compound being walked starts with a list
                // pseudo-class (`:is()`, `:where()`, `:not()`, `:has()`).
                let mut leads_with_list = false;
                let mut first = true;
                let mut sel: Option<Selector<'_>> = list.first();
                while let Some(s) = sel {
                    visited += 1;
                    if visited > 256 {
                        return None;
                    }
                    let combinator = s.combinator();
                    if first || combinator != Combinator::Close {
                        // The compound before this one is searched over every
                        // ancestor / preceding sibling when this one attaches
                        // by a descendant or `~` combinator.
                        if !first && leads_with_list && is_multi(combinator, false) {
                            shape.departs = true;
                        }
                        leads_with_list = matches!(
                            s.simple(),
                            Simple::PseudoClassFunction(FunctionArg::Selectors { .. })
                        );
                        // In a `:has()` argument the search runs forward: this
                        // compound is looked for over every child, descendant
                        // or following sibling.
                        if leads_with_list && in_has && is_multi(combinator, true) {
                            shape.departs = true;
                        }
                    }
                    first = false;
                    shape.simple(s, in_has, &mut work);
                    sel = s.next();
                }
            }
        }
        Some(shape)
    }

    fn simple<'p>(&mut self, s: Selector<'p>, in_has: bool, work: &mut Vec<(Lists<'p>, bool)>) {
        match s.combinator() {
            Combinator::Descendant | Combinator::SubsequentSibling => self.backtracks += 1,
            Combinator::Column | Combinator::Other => self.departs = true,
            _ => {}
        }
        let upper = s.name().iter().any(u8::is_ascii_uppercase);
        match s.simple() {
            Simple::Type | Simple::Attribute(_) => {
                self.names = true;
                self.upper_names |= upper;
            }
            Simple::PseudoClass(
                PseudoClass::Checked
                | PseudoClass::Disabled
                | PseudoClass::Enabled
                | PseudoClass::ReadOnly
                | PseudoClass::ReadWrite,
            ) => self.departs = true,
            Simple::PseudoClassFunction(arg) => match arg {
                FunctionArg::Nth { anb, .. } => {
                    if let Some(anb) = anb {
                        if anb.of.is_some()
                            || anb.a.unsigned_abs() >= ANB_EXACT
                            || anb.b.unsigned_abs() >= ANB_EXACT
                        {
                            self.departs = true;
                        }
                    }
                }
                FunctionArg::Selectors { pseudo, lists } => {
                    let has = pseudo == ListPseudo::Has;
                    if has {
                        self.backtracks += 1;
                    }
                    work.push((lists, in_has || has));
                }
                // Both raised by the port and matched by Lexbor.
                FunctionArg::Contains(_) | FunctionArg::Current(_) => self.departs = true,
                FunctionArg::Other => {}
            },
            _ => {}
        }
    }
}

/// Whether a compound reached through `combinator` has several candidates
/// to try: an ancestor or preceding sibling at any distance - or, searching
/// forward from a `:has()` subject, any child too.
fn is_multi(combinator: Combinator, forward: bool) -> bool {
    match combinator {
        Combinator::Descendant | Combinator::SubsequentSibling => true,
        Combinator::Child => forward,
        _ => false,
    }
}

/// Whether the two engines are meant to give the same answer (module doc).
fn comparable(shape: &Shape, doc: &Doc) -> bool {
    if shape.departs {
        return false;
    }
    if doc.foreign && shape.names && (doc.foreign_names || shape.upper_names) {
        return false;
    }
    true
}

/// Whether the old engine's exhaustive backtracking stays within a run.
fn affordable(shape: &Shape, doc: &Doc) -> bool {
    let n = doc.elements.max(2) as f64;
    n.powi(shape.backtracks as i32 + 1) <= OLD_ENGINE_WORK
}
