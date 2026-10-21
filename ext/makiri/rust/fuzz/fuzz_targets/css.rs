//! CSS selectors over ONE fixed XML document: the matcher (`css_match`) over
//! the XML arena, which answers `Makiri::XML`'s `css` / `at_css` /
//! `matches?`, held to itself and to the lowering it replaced.
//!
//! The first input byte picks whether the bindings name a default namespace
//! (the ns Hash's `"xmlns"` entry); the rest is the selector. Each selector is
//! run from the document node (where a type selector is answered from the
//! name indexes) and from the root element (a walk), and the three entry
//! points must agree: `select_first` is the first of `select_all`, and
//! `matches_any` holds exactly for its members.
//!
//! The lowering (`crate::css`, the `xml-css-reference` feature) still runs
//! where it answers, and where the two mean the same it must answer alike:
//! the lowering reads a bare type selector as no namespace (or the default
//! one) and a bare `*` as any element, the matcher as a browser does - so a
//! selector with a bare type selector is compared only under a default
//! namespace, and one with a bare `*` only without.
#![no_main]

use libfuzzer_sys::fuzz_target;

mod common;
use common::*;
use makiri::css::{compile_owned, CssNs, Form};
use makiri::css_match::dom::CssNode;
use makiri::css_match::{matches_any, select_all, select_first, Namespaces, Scratch};
use makiri::selectors::ast::{Combinator, FunctionArg, Lists, PseudoClass, Simple};
use makiri::xml::css::{XmlHost, XmlNode};
use makiri::xml::NodeId;

const FIXED_XML: &[u8] = b"<?xml version='1.0'?>\
<root xmlns='http://example.com/default' xmlns:ns='http://example.com/ns'>\
  <a id='x' class='c1 c2' lang='en-US' title='t'>text1</a>\
  <b id='y'><c/><c class='c1'/><d/><c/>tail</b>\
  <ns:e ns:attr='v'><c/>namespaced</ns:e>\
  <e xmlns='' attr='w'><c/></e>\
  <!-- comment -->\
  <?pi target='value'?>\
</root>";

const NS: &[u8] = b"http://example.com/ns";
const DEFAULT: &[u8] = b"http://example.com/default";

/// Whether the lowering is not compared on the selector: it holds a type
/// (`bare_type`) or universal selector written without a namespace prefix,
/// which the two read differently; under a default namespace (`!bare_type`)
/// a compound with neither, which the matcher keeps to that namespace
/// (Selectors 4 §5.3) and the lowering does not; or `:empty`, which the
/// lowering counts as Lexbor does and the matcher as the browsers do (M10).
/// Walked on a work list, as the matcher compiles it, so no input nests the
/// harness's stack.
fn not_comparable(groups: Lists<'_>, bare_type: bool) -> bool {
    let mut pending = vec![groups];
    while let Some(lists) = pending.pop() {
        for list in lists {
            let mut sel = list.first();
            let mut starts = true;
            while let Some(s) = sel {
                if starts && !bare_type && !matches!(s.simple(), Simple::Type | Simple::Universal) {
                    return true;
                }
                starts = s.next().is_some_and(|n| n.combinator() != Combinator::Close);
                match s.simple() {
                    Simple::Type | Simple::Universal => {
                        if s.ns().is_none() && (bare_type == matches!(s.simple(), Simple::Type)) {
                            return true;
                        }
                    }
                    Simple::PseudoClass(PseudoClass::Empty) => return true,
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

/// The lowering's answer from the document node, or `None` where it refuses.
fn lowered(doc: &Document, text: VerifiedText, default: bool) -> Option<Vec<usize>> {
    let mut ctx = xml_context(doc);
    let l = ctx.limits_mut();
    l.max_eval_ops = 1_000_000;
    l.max_nodeset_size = 10_000;
    ctx.register_ns(b"ns", NS).ok()?;
    if default {
        ctx.register_ns(b"xmlns", DEFAULT).ok()?;
    }
    let ns = CssNs {
        default_namespace: default,
    };
    let mut budget = Budget::with_limits(ctx.limits());
    let ast = compile_owned(text, &ns, Form::Select, &mut budget).ok()?;
    match ctx.evaluate(&ast, None).ok()? {
        XPathValue::NodeSet(set) => {
            Some(set.as_slice().iter().map(|t| t.as_ptr() as usize).collect())
        }
        _ => None,
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, selector)) = data.split_first() else {
        return;
    };
    let Ok(doc) = xml_parse(FIXED_XML) else {
        return;
    };
    let Some(expr) = Expr::new(selector) else {
        return;
    };
    let Some(text) = expr.text() else {
        return;
    };
    let Ok(parsed) = makiri::selectors::parse(text.as_bytes()) else {
        return;
    };
    let groups = parsed.groups();
    let default = mode & 1 == 1;
    let bindings: [(&[u8], &[u8]); 1] = [(b"ns", NS)];
    let ns = Namespaces::new(&bindings, default.then_some(DEFAULT));
    let mut scratch: Scratch<XmlHost> = Scratch::new();

    let docnode = XmlNode::new(&doc, doc.doc_node());
    let Ok(all) = select_all(&mut scratch, docnode, groups, &ns) else {
        return;
    };
    if let Ok(first) = select_first(&mut scratch, docnode, groups, &ns) {
        assert!(
            first.map(XmlNode::id) == all.first().map(|n| n.id()),
            "select_first is not the first of select_all"
        );
    }
    let mut n = docnode;
    while let Some(next) = n.preorder_next(docnode) {
        n = next;
        if n.element().is_some() {
            if let Ok(one) = matches_any(&mut scratch, groups, &ns, n) {
                assert_eq!(
                    one,
                    all.contains(&n),
                    "matches_any disagrees with select_all"
                );
            }
        }
    }
    /* From the root element: a walk, where the document node's type
     * selectors come from the indexes. */
    if let Some(top) = doc.root().map(|id| XmlNode::new(&doc, id)) {
        if let (Ok(below), Ok(at_top)) = (
            select_all(&mut scratch, top, groups, &ns),
            matches_any(&mut scratch, groups, &ns, top),
        ) {
            let walked: Vec<NodeId> = at_top
                .then_some(top.id())
                .into_iter()
                .chain(below.iter().map(|n| n.id()))
                .collect();
            let indexed: Vec<NodeId> = all.iter().map(|n| n.id()).collect();
            assert_eq!(walked, indexed, "the index and the walk disagree");
        }
    }

    if not_comparable(groups, !default) {
        return;
    }
    if let Some(old) = lowered(&doc, text, default) {
        let new: Vec<usize> = all.iter().map(|n| n.id().to_token()).collect();
        assert_eq!(new, old, "the matcher and the lowering disagree");
    }
});
