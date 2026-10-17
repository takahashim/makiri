//! The arena DOM (`crate::html::dom`) against the Lexbor adapter it stands in
//! for: a parsed Lexbor document is copied into the arena, and every read the
//! adapter answers is asked of both, node by node.
//!
//! Positions stand in for identities: both documents are walked in the same
//! order (template contents included, each element followed by its
//! attributes), and a node one side answers is compared by its place in that
//! walk with the node the other side answers.

#![allow(unsafe_code)]
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;

use crate::conformance::dat::{self, Scripting};
use crate::conformance::tree_construction_dir;
use crate::html::dom as arena;
use crate::lexbor::adapter::html as lex;
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::adapter::tree_guard::DepthLimit;

/// Every node of the walk: pre-order with template contents, each element
/// followed by its attributes.
fn lex_walk(doc: lex::HtmlDoc<'_>) -> Vec<lex::HtmlNode<'_>> {
    let mut out = Vec::new();
    for n in doc.as_node().subtree_with_contents() {
        out.push(n);
        if let Some(e) = n.element() {
            out.extend(e.attrs().map(lex::HtmlAttr::node));
        }
    }
    out
}

fn arena_walk(doc: arena::HtmlDoc<'_>) -> Vec<arena::HtmlNode<'_>> {
    let mut out = Vec::new();
    for n in doc.as_node().subtree_with_contents() {
        out.push(n);
        if let Some(e) = n.element() {
            out.extend(e.attrs().map(arena::HtmlAttr::node));
        }
    }
    out
}

/// The two walks, with each node's position.
struct Pair<'l, 'a> {
    l: Vec<lex::HtmlNode<'l>>,
    a: Vec<arena::HtmlNode<'a>>,
    lpos: HashMap<usize, usize>,
    apos: HashMap<arena::NodeId, usize>,
}

impl<'l, 'a> Pair<'l, 'a> {
    fn new(ld: lex::HtmlDoc<'l>, ad: arena::HtmlDoc<'a>) -> Self {
        let (l, a) = (lex_walk(ld), arena_walk(ad));
        assert_eq!(l.len(), a.len(), "the walks differ in length");
        let lpos = l
            .iter()
            .enumerate()
            .map(|(i, n)| (lex::RawNode::from(*n).as_ptr() as usize, i))
            .collect();
        let apos = a.iter().enumerate().map(|(i, n)| (n.id(), i)).collect();
        Pair { l, a, lpos, apos }
    }

    fn lp(&self, n: Option<lex::HtmlNode<'_>>) -> Option<usize> {
        n.map(|n| self.lpos[&(lex::RawNode::from(n).as_ptr() as usize)])
    }

    fn ap(&self, n: Option<arena::HtmlNode<'_>>) -> Option<usize> {
        n.map(|n| self.apos[&n.id()])
    }
}

fn s(b: Option<&[u8]>) -> Option<String> {
    b.map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Every read the adapter answers, asked of both documents.
fn compare(input: &[u8]) {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).expect("Lexbor parses");
    // SAFETY: `parsed` owns the document and outlives every handle read here.
    let ld = unsafe { parsed.raw_doc().as_doc() };
    let copy = crate::lexbor::adapter::arena_copy::copy_document(ld).expect("the copy");
    let ad = arena::HtmlDoc::of(&copy);
    let p = Pair::new(ld, ad);
    let ctx = String::from_utf8_lossy(input);

    assert_eq!(ld.compat_mode(), ad.compat_mode(), "{ctx}");
    assert_eq!(s(ld.title()), s(ad.title().as_deref()), "title of {ctx}");
    for name in [
        &b"div"[..],
        b"DIV",
        b"svg",
        b"foreignObject",
        b"foreignobject",
        b"my-el",
        b"x:y",
        b"",
    ] {
        assert_eq!(
            ld.tag_id(name).map(|t| t.static_index()),
            ad.tag_id(name).map(|t| t.static_index()),
            "tag_id({}) in {ctx}",
            String::from_utf8_lossy(name)
        );
    }

    for (i, (&l, &a)) in p.l.iter().zip(&p.a).enumerate() {
        let what = format!(
            "node {i} ({:?} {:?}) of {ctx}",
            l.node_type(),
            s(Some(l.node_name()))
        );
        assert_eq!(l.node_type(), a.node_type(), "{what}");
        assert_eq!(
            s(Some(l.node_name())),
            s(Some(a.node_name())),
            "node_name: {what}"
        );
        assert_eq!(
            s(Some(l.qualified_name())),
            s(Some(a.qualified_name())),
            "qualified_name: {what}"
        );
        assert_eq!(s(l.ns_uri()), s(a.ns_uri()), "ns: {what}");
        assert_eq!(
            l.ns_id().map(|n| n.is_static()),
            a.ns_id().map(|n| n.is_static()),
            "ns static: {what}"
        );
        assert_eq!(l.has_prefix(), a.has_prefix(), "prefix: {what}");
        assert_eq!(
            l.tag_id().and_then(|t| t.static_index()),
            a.tag_id().and_then(|t| t.static_index()),
            "tag: {what}"
        );
        assert_eq!(
            l.tag_id().is_some(),
            a.tag_id().is_some(),
            "tag set: {what}"
        );
        assert_eq!(l.source_offset(), a.source_offset(), "offset: {what}");
        assert_eq!(s(l.pi_target()), s(a.pi_target()), "pi: {what}");
        assert_eq!(s(l.doctype_public_id()), s(a.doctype_public_id()), "{what}");
        assert_eq!(s(l.doctype_system_id()), s(a.doctype_system_id()), "{what}");
        assert_eq!(s(l.char_data()), s(a.char_data()), "{what}");
        assert_eq!(s(l.data()), s(a.data()), "{what}");
        assert_eq!(s(l.own_text()), s(a.own_text()), "{what}");
        assert_eq!(l.is_html_template(), a.is_html_template(), "{what}");
        for (rel, lv, av) in [
            ("parent", p.lp(l.parent()), p.ap(a.parent())),
            ("first_child", p.lp(l.first_child()), p.ap(a.first_child())),
            ("last_child", p.lp(l.last_child()), p.ap(a.last_child())),
            ("next", p.lp(l.next()), p.ap(a.next())),
            ("prev", p.lp(l.prev()), p.ap(a.prev())),
            (
                "template_content",
                p.lp(l.template_content()),
                p.ap(a.template_content()),
            ),
            (
                "fragment_host",
                p.lp(l.fragment_host()),
                p.ap(a.fragment_host()),
            ),
            (
                "document_root",
                p.lp(l.document_root()),
                p.ap(a.document_root()),
            ),
        ] {
            assert_eq!(lv, av, "{rel}: {what}");
        }

        if let (Some(le), Some(ae)) = (l.element(), a.element()) {
            assert_eq!(
                s(Some(le.qualified_name())),
                s(Some(ae.qualified_name())),
                "{what}"
            );
            assert_eq!(le.has_written_name(), ae.has_written_name(), "{what}");
            assert_eq!(s(Some(le.local_name())), s(Some(ae.local_name())), "{what}");
            assert_eq!(
                s(Some(le.dom_local_name())),
                s(Some(ae.dom_local_name())),
                "{what}"
            );
            assert_eq!(s(le.tag_name()), s(ae.tag_name()), "tag_name: {what}");
            assert_eq!(
                p.lp(le.id_attr().map(lex::HtmlAttr::node)),
                p.ap(ae.id_attr().map(arena::HtmlAttr::node)),
                "id_attr: {what}"
            );
            assert_eq!(
                p.lp(le.class_attr().map(lex::HtmlAttr::node)),
                p.ap(ae.class_attr().map(arena::HtmlAttr::node)),
                "class_attr: {what}"
            );
            /* Every lookup, by every name an attribute here answers to, and
             * by a few that it does not. */
            let mut names: Vec<Vec<u8>> = vec![b"id".to_vec(), b"ID".to_vec(), b"href".to_vec()];
            for la in le.attrs() {
                names.push(la.qualified_name().to_vec());
                names.push(la.qualified_name().to_ascii_uppercase());
                names.push(la.local_name().to_vec());
                names.push(la.dom_local_name().to_vec());
            }
            for name in &names {
                let nm = String::from_utf8_lossy(name);
                assert_eq!(
                    p.lp(le.attr_by_name(name).map(lex::HtmlAttr::node)),
                    p.ap(ae.attr_by_name(name).map(arena::HtmlAttr::node)),
                    "attr_by_name({nm}): {what}"
                );
                let (lr, ar) = (ld.resolve_attr_name(name), ad.resolve_attr_name(name));
                assert_eq!(
                    p.lp(le.attr_by_resolved_name(name, lr).map(lex::HtmlAttr::node)),
                    p.ap(ae
                        .attr_by_resolved_name(name, ar)
                        .map(arena::HtmlAttr::node)),
                    "attr_by_resolved_name({nm}): {what}"
                );
                assert_eq!(
                    s(le.get_attribute(name)),
                    s(ae.get_attribute(name)),
                    "{what}"
                );
                for uri in [
                    None,
                    Some(&b"http://www.w3.org/1999/xlink"[..]),
                    Some(b"urn:x"),
                ] {
                    let lns = uri.map(|u| ld.lookup_ns(u));
                    let ans = uri.map(|u| ad.lookup_ns(u));
                    assert_eq!(
                        lns.is_some_and(|n| n.is_none()),
                        ans.is_some_and(|n| n.is_none())
                    );
                    if lns.is_some_and(|n| n.is_none()) {
                        continue;
                    }
                    assert_eq!(
                        p.lp(le.attr_by_ns(lns.flatten(), name).map(lex::HtmlAttr::node)),
                        p.ap(ae
                            .attr_by_ns(ans.flatten(), name)
                            .map(arena::HtmlAttr::node)),
                        "attr_by_ns({uri:?}, {nm}): {what}"
                    );
                }
            }
        }
        if let (Some(la), Some(aa)) = (l.attr(), a.attr()) {
            assert_eq!(
                s(Some(la.qualified_name())),
                s(Some(aa.qualified_name())),
                "{what}"
            );
            assert_eq!(s(Some(la.local_name())), s(Some(aa.local_name())), "{what}");
            assert_eq!(
                s(Some(la.dom_local_name())),
                s(Some(aa.dom_local_name())),
                "{what}"
            );
            assert_eq!(s(Some(la.value())), s(Some(aa.value())), "{what}");
            assert_eq!(s(la.own_ns_uri()), s(aa.own_ns_uri()), "own_ns: {what}");
            assert_eq!(
                p.lp(la.owner().map(lex::HtmlElement::node)),
                p.ap(aa.owner().map(arena::HtmlElement::node)),
                "{what}"
            );
        }
    }

    /* Document order, over a spread of pairs. */
    let n = p.l.len();
    let step = (n / 40).max(1);
    for i in (0..n).step_by(step) {
        for j in (0..n).step_by(step) {
            assert_eq!(
                p.l[i].document_order(p.l[j]),
                p.a[i].document_order(p.a[j]),
                "document_order({i}, {j}) of {ctx}"
            );
        }
    }
}

const SAMPLE: &[u8] = b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\" \"http://www.w3.org/TR/html4/strict.dtd\">\
    <title>  The \t\n Title  </title>\
    <svg viewBox='0 0 1 1' xmlns:q=u q:r=s><foreignObject><p id=x class=y ID=z>p</p></foreignObject>\
    <a xlink:href=z xml:lang=en></a><linearGradient gradientUnits=u /></svg>\
    <math definitionURL=d><mi>x</mi></math><template><b>t</b><my-el X=1></my-el></template>\
    <x:y a:b=c></x:y><!--c--><?pi d?><table><tr><td>1</table>";

#[test]
fn the_arena_reads_as_lexbor_does() {
    compare(SAMPLE);
    compare(b"<title></title>");
    compare(b"<title><b>x</b></title>");
    compare(b"<p>no title</p><svg><title>svg title</title></svg>");
    compare(b"<title> a  b </title><title>second</title>");
}

#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn the_arena_reads_as_lexbor_does_on_wpt_inputs() {
    let tests = dat::read_dir(&tree_construction_dir()).expect("WPT data");
    for t in tests
        .iter()
        .filter(|t| t.fragment.is_none() && t.scripting != Scripting::On)
    {
        compare(&t.data);
    }
}
