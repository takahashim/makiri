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

/// Every read the adapter answers, asked of a parse of `input` and of its
/// arena copy.
fn compare(input: &[u8]) {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).expect("Lexbor parses");
    // SAFETY: `parsed` owns the document and outlives every handle read here.
    let ld = unsafe { parsed.raw_doc().as_doc() };
    let copy = crate::lexbor::adapter::arena_copy::copy_document(ld).expect("the copy");
    compare_docs(
        ld,
        arena::HtmlDoc::of(&copy),
        &String::from_utf8_lossy(input),
    );
}

/// Every read the adapter answers, asked of both documents.
fn compare_docs(ld: lex::HtmlDoc<'_>, ad: arena::HtmlDoc<'_>, ctx: &str) {
    let p = Pair::new(ld, ad);

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
        /* Lexbor's `lxb_dom_attr_set_name_ns` stores the name entry's
         * POINTER as a prefixed attribute's local name, not its id, so a
         * static name reads as an appended one there. Every name read
         * resolves either to the same text, and a lookup by name compares
         * the qualified name, which a prefixed attribute's never equals
         * alone; the arena keeps the id. */
        if !(l.attr().is_some() && l.has_prefix()) {
            assert_eq!(
                l.tag_id().and_then(|t| t.static_index()),
                a.tag_id().and_then(|t| t.static_index()),
                "tag: {what}"
            );
        }
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

    /* Serialization, every form, of every node Lexbor serializes. */
    for (i, (&l, &a)) in p.l.iter().zip(&p.a).enumerate() {
        for (deep, pretty) in [(false, false), (true, false), (false, true), (true, true)] {
            let lo = crate::lexbor::serialize::serialize(l, deep, pretty);
            let ao = arena::serialize::serialize_buf(a, deep, pretty);
            assert_eq!(
                lo.as_ref()
                    .map(|b| String::from_utf8_lossy(b.as_slice()).into_owned()),
                ao.as_ref()
                    .map(|b| String::from_utf8_lossy(b.as_slice()).into_owned()),
                "serialize(deep {deep}, pretty {pretty}) of node {i} ({:?}) of {ctx}",
                l.node_type()
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

/* ---------- edits ---------- */

static PLACED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// xorshift64*, seeded, so a failure names its seed and step.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

const NAMES: &[&[u8]] = &[
    b"id",
    b"ID",
    b"class",
    b"Class",
    b"href",
    b"data-x",
    b"viewBox",
    b"xlink:href",
    b"p:id",
    b"q:Foo",
];
const NSS: &[Option<&[u8]>] = &[
    None,
    Some(b"http://www.w3.org/1999/xhtml"),
    Some(b"http://www.w3.org/2000/svg"),
    Some(b"http://www.w3.org/1999/xlink"),
    Some(b"urn:Mixed:Case"),
];
const TAGS: &[&[u8]] = &[
    b"div",
    b"DIV",
    b"template",
    b"Template",
    b"my-el",
    b"br",
    b"BR",
    b"linearGradient",
    b"svg",
    b"p",
];
const PREFIXES: &[&[u8]] = &[b"", b"", b"x", b"svg", b"Q"];
const TEXTS: &[&[u8]] = &[b"", b"t", b"two words", b"a\0b", b"<&>"];

/// A node of each side at the same walk position.
fn at<'l, 'a>(
    ld: lex::HtmlDoc<'l>,
    ad: arena::HtmlDoc<'a>,
    i: usize,
) -> (lex::HtmlNode<'l>, arena::HtmlNode<'a>) {
    let (l, a) = (lex_walk(ld), arena_walk(ad));
    assert_eq!(l.len(), a.len());
    let i = i % l.len();
    (l[i], a[i])
}

/// `steps` random edits on a parse of `input` and on its arena copy, all
/// reads compared after each. `other` is a second document both sides import
/// from.
fn edit_sequence(input: &[u8], other: &[u8], seed: u64, steps: usize) {
    use crate::lexbor::adapter::html::HtmlDocMut as LexMut;
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).expect("Lexbor parses");
    let other_parsed = parse_html(other, false, DepthLimit::UNLIMITED).expect("Lexbor parses");
    // SAFETY: each parse owns its document and outlives every handle here;
    // only this test reads or writes them.
    let (ld, lo) = unsafe { (parsed.raw_doc().as_doc(), other_parsed.raw_doc().as_doc()) };
    let mut ac = crate::lexbor::adapter::arena_copy::copy_document(ld).expect("copy");
    let ao_doc = crate::lexbor::adapter::arena_copy::copy_document(lo).expect("copy");
    let ao = arena::HtmlDoc::of(&ao_doc);
    // SAFETY: as above; the test is the only editor.
    let mut lm = unsafe { LexMut::assume_exclusive(ld) };
    let mut rng = Rng(seed);
    for step in 0..steps {
        let ctx = format!("seed {seed:#x} step {step}");
        let op = rng.below(13);
        let i = rng.below(1 << 20);
        let mut am = arena::HtmlDocMut::new(&mut ac);
        let (ln, an) = at(ld, am.read(), i);
        let (lr, ar) = (lex::RawNode::from(ln), arena::RawNode::from(an));
        let mut made: Option<(lex::RawNode, arena::RawNode)> = None;
        match op {
            0 => {
                let (name, value) = (*rng.pick(NAMES), *rng.pick(TEXTS));
                let l = lm.set_attribute(lr, name, value).is_ok();
                let a = am.set_attribute(ar, name, value).is_ok();
                assert_eq!(l, a, "set_attribute: {ctx}");
            }
            1 => {
                let (ns, name, value) = (*rng.pick(NSS), *rng.pick(NAMES), *rng.pick(TEXTS));
                let l = lm.set_attribute_ns(lr, ns, name, value).is_ok();
                let a = am.set_attribute_ns(ar, ns, name, value).is_ok();
                assert_eq!(l, a, "set_attribute_ns: {ctx}");
            }
            2 => {
                let name = *rng.pick(NAMES);
                lm.remove_attribute(lr, name);
                am.remove_attribute(ar, name);
            }
            3 => {
                lm.remove_from_owner(lr);
                am.remove_from_owner(ar);
            }
            4 => {
                let t = *rng.pick(TEXTS);
                let l = lm.set_text_content(lr, t).is_ok();
                let a = am.set_text_content(ar, t).is_ok();
                assert_eq!(l, a, "set_text_content: {ctx}");
            }
            5 => {
                let t = *rng.pick(TAGS);
                made = lm.create_element(t).zip(am.create_element(t));
            }
            6 => {
                let (t, ns, p) = (
                    *rng.pick(TAGS),
                    rng.pick(NSS).unwrap_or_default(),
                    *rng.pick(PREFIXES),
                );
                let l = lm.create_element_ns(t, ns, p);
                let a = am.create_element_ns(t, ns, p);
                assert_eq!(l.is_some(), a.is_some(), "create_element_ns: {ctx}");
                made = l.zip(a);
            }
            7 => {
                let t = *rng.pick(TEXTS);
                made = match rng.below(5) {
                    0 => lm.create_text(t).zip(am.create_text(t)),
                    1 => lm.create_comment(t).zip(am.create_comment(t)),
                    2 => {
                        let data: &[u8] = if rng.below(2) == 0 { t } else { b"x?>y" };
                        let l = lm.create_pi(b"pi", data);
                        let a = am.create_pi(b"pi", data);
                        assert_eq!(l.is_some(), a.is_some(), "create_pi: {ctx}");
                        l.zip(a)
                    }
                    3 => lm
                        .create_doctype(b"Html", Some(t), None)
                        .zip(am.create_doctype(b"Html", Some(t), None)),
                    _ => {
                        let f = lm.create_fragment().zip(am.create_fragment());
                        if let Some((lf, af)) = f {
                            for _ in 0..rng.below(3) {
                                let c = lm.create_text(b"f").zip(am.create_text(b"f"));
                                if let Some((lc, ac2)) = c {
                                    lm.place(lf, lc, lex::Place::Child);
                                    am.place(af, ac2, arena::Place::Child);
                                }
                            }
                        }
                        f
                    }
                };
            }
            8 => {
                let deep = rng.below(2) == 0;
                if ln.node_type() != crate::node_type::NodeType::Document {
                    let l = lm.copy(lr, deep);
                    let a = am.copy(ar, deep);
                    assert_eq!(l.is_ok(), a.is_ok(), "copy: {ctx}");
                    made = l.ok().zip(a.ok());
                }
            }
            9 => {
                let j = rng.below(1 << 20);
                let (lsrc, asrc) = at(lo, ao, j);
                if lsrc.node_type() != crate::node_type::NodeType::Document {
                    let deep = rng.below(2) == 0;
                    let l = lm.import(lsrc, deep);
                    let a = am.import(asrc, deep);
                    assert_eq!(l.is_ok(), a.is_ok(), "import: {ctx}");
                    made = l.ok().zip(a.ok());
                }
            }
            10 => {
                if ln.parent().is_some() && ln.attr().is_none() {
                    lm.detach(lr);
                    am.detach(ar);
                }
            }
            _ => {
                /* Move an existing node. */
                let j = rng.below(1 << 20);
                let (lt, at2) = at(ld, am.read(), j);
                made = Some((lex::RawNode::from(lt), arena::RawNode::from(at2)))
                    .filter(|_| lt.attr().is_none());
            }
        }
        if let Some((lnew, anew)) = made {
            /* Put it somewhere, if the DOM allows it there. */
            let k = rng.below(1 << 20);
            let place = *rng.pick(&[0usize, 1, 2, 3]);
            let (lt, atg) = at(ld, am.read(), k);
            let (lp, ap) = match place {
                0 => (lex::Place::Child, arena::Place::Child),
                1 => (lex::Place::Before, arena::Place::Before),
                2 => (lex::Place::After, arena::Place::After),
                _ => (lex::Place::Replace, arena::Place::Replace),
            };
            let lc = lex::Insertion::new(lt, lp, lm.node(lnew)).and_then(|x| x.check());
            let ac2 = {
                let r = am.read();
                arena::Insertion::new(atg, ap, r.node(anew)).and_then(|x| x.check())
            };
            let (lt, atg) = (lex::RawNode::from(lt), arena::RawNode::from(atg));
            assert_eq!(lc, ac2, "insertion check: {ctx}");
            if lc.is_ok() {
                PLACED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if lm.node(lnew).parent().is_some() {
                    lm.detach(lnew);
                    am.detach(anew);
                }
                lm.place(lt, lnew, lp);
                am.place(atg, anew, ap);
            }
        }
        compare_docs(ld, am.read(), &ctx);
    }
}

const EDIT_INPUT: &[u8] = b"<!DOCTYPE html><title>t</title><div id=a class=b><p>one<b>two</b></p>\
    <svg viewBox='0 0 1 1'><a xlink:href=z></a></svg><template><i>x</i></template><!--c--></div>";
const EDIT_OTHER: &[u8] = b"<svg xmlns:q=urn:Q q:x=1><linearGradient/><foreignObject><b id=z>z</b>\
    </foreignObject></svg><template><u>u</u></template><my-el data-x=1>m</my-el>";

/// `ARENA_SEEDS` runs more sequences than the default 40.
#[test]
fn edits_leave_the_arena_reading_as_lexbor_does() {
    let seeds: u64 = std::env::var("ARENA_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    for seed in 1..=seeds {
        edit_sequence(
            EDIT_INPUT,
            EDIT_OTHER,
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            60,
        );
    }
    /* The sequences must actually move nodes around, not only fail checks. */
    assert!(PLACED.load(core::sync::atomic::Ordering::Relaxed) > seeds as usize * 5);
}

/* ---------- fragments ---------- */

/// `input` parsed as a fragment in a `context` element (`svg g`, `td`, ...)
/// placed in a document's body, on both sides, the result put into that
/// element, and everything compared - then the same input parsed in a named
/// context (`Document#fragment`) and appended to the body.
fn fragment_case(input: &[u8], context: &str) {
    use crate::lexbor::adapter::html::HtmlDocMut as LexMut;
    use crate::lexbor::arena_fragment as af;
    use crate::lexbor::fragment as lf;
    let parsed = parse_html(b"<body>", false, DepthLimit::UNLIMITED).expect("parses");
    // SAFETY: `parsed` owns the document; only this test reads or writes it.
    let ld = unsafe { parsed.raw_doc().as_doc() };
    let mut ac = crate::lexbor::adapter::arena_copy::copy_document(ld).expect("copy");
    // SAFETY: as above.
    let mut lm = unsafe { LexMut::assume_exclusive(ld) };
    let mut am = arena::HtmlDocMut::new(&mut ac);
    let (ns, name): (&[u8], &str) = match context.split_once(' ') {
        Some(("svg", local)) => (b"http://www.w3.org/2000/svg", local),
        Some(("math", local)) => (b"http://www.w3.org/1998/Math/MathML", local),
        _ => (b"http://www.w3.org/1999/xhtml", context),
    };
    let ctx = format!("fragment {:?} in {context}", String::from_utf8_lossy(input));
    let body = |w: &[lex::HtmlNode<'_>]| {
        w.iter()
            .position(|n| n.element().is_some_and(|e| e.local_name() == b"body"))
            .expect("a body")
    };
    let bi = body(&lex_walk(ld));
    let (lbody, abody) = at(ld, am.read(), bi);
    let (lbody, abody) = (lex::RawNode::from(lbody), arena::RawNode::from(abody));

    /* An element context. */
    let lc = lm
        .create_element_ns(name.as_bytes(), ns, b"")
        .expect("context");
    let ac2 = am
        .create_element_ns(name.as_bytes(), ns, b"")
        .expect("context");
    lm.place(lbody, lc, lex::Place::Child);
    am.place(abody, ac2, arena::Place::Child);
    let lf_ = unsafe {
        // SAFETY: the context element is live for the call.
        lf::TransientFragment::parse(
            input,
            false,
            &lf::FragmentContext::Element(lc),
            DepthLimit::UNLIMITED,
        )
    };
    let actx = af::FragmentContext::element(am.read(), ac2).expect("context");
    let af_ = af::TransientFragment::parse(input, false, &actx, DepthLimit::UNLIMITED);
    assert_eq!(lf_.is_ok(), af_.is_ok(), "parse: {ctx}");
    if let (Ok(l), Ok(a)) = (lf_, af_) {
        let (lfr, afr) = (
            lm.create_fragment().expect("f"),
            am.create_fragment().expect("f"),
        );
        l.import_into_doc(&mut lm, lfr).expect("import");
        a.import_into_doc(&mut am, afr).expect("import");
        lm.place(lc, lfr, lex::Place::Child);
        am.place(ac2, afr, arena::Place::Child);
    }
    compare_docs(ld, am.read(), &ctx);

    /* A named context. */
    let lex_at = match context.split_once(' ') {
        Some(("svg", _)) => lf::FragmentTag::SVG,
        Some(("math", _)) => lf::FragmentTag::MATH,
        _ => match ld.tag_id(name.as_bytes()) {
            Some(t) => lf::FragmentTag::html(t),
            None => return,
        },
    };
    let arena_at = match context.split_once(' ') {
        Some(("svg", _)) => af::FragmentTag::SVG,
        Some(("math", _)) => af::FragmentTag::MATH,
        _ => af::FragmentTag::html(am.read().tag_id(name.as_bytes()).expect("same tags")),
    };
    let lf_ = unsafe {
        // SAFETY: the document is live for the call.
        lf::TransientFragment::parse(
            input,
            false,
            &lf::FragmentContext::Tag {
                doc: parsed.raw_doc(),
                at: lex_at,
            },
            DepthLimit::UNLIMITED,
        )
    };
    let af_ = af::TransientFragment::parse(
        input,
        false,
        &af::FragmentContext::tag(arena_at),
        DepthLimit::UNLIMITED,
    );
    assert_eq!(lf_.is_ok(), af_.is_ok(), "named parse: {ctx}");
    if let (Ok(l), Ok(a)) = (lf_, af_) {
        let (lfr, afr) = (
            lm.create_fragment().expect("f"),
            am.create_fragment().expect("f"),
        );
        l.import_into_doc(&mut lm, lfr).expect("import");
        a.import_into_doc(&mut am, afr).expect("import");
        lm.place(lbody, lfr, lex::Place::Child);
        am.place(abody, afr, arena::Place::Child);
    }
    compare_docs(ld, am.read(), &format!("named {ctx}"));
}

#[test]
fn fragments_read_as_lexbor_does() {
    fragment_case(b"<td>x</td><option>o", "tr");
    fragment_case(b"<g><desc><b>x</b></desc></g>", "svg g");
    fragment_case(b"<mi>x</mi><b>y", "math mi");
    fragment_case(b"<template><p>t</p></template>&amp;", "div");
    fragment_case(b"</textarea>x", "textarea");
    fragment_case(b"<option>a<option>b", "select");
}

#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn fragments_read_as_lexbor_does_on_wpt_inputs() {
    let tests = dat::read_dir(&tree_construction_dir()).expect("WPT data");
    for t in tests.iter().filter(|t| t.scripting != Scripting::On) {
        if let Some(context) = &t.fragment {
            fragment_case(&t.data, context);
        }
    }
}

/// The option guard reaches through the context's ancestors on both sides,
/// and a context in a custom namespace gives it to what inherits it.
#[test]
fn fragment_contexts_resolve_alike() {
    use crate::lexbor::adapter::html::HtmlDocMut as LexMut;
    use crate::lexbor::arena_fragment as af;
    use crate::lexbor::fragment as lf;
    let many = b"<option>".repeat(crate::lexbor::adapter::tree_guard::MAX_SELECT_OPTIONS + 1);
    let chains: [&[&[u8]]; 3] = [&[b"select", b"optgroup"], &[b"select", b"div"], &[b"div"]];
    for chain in chains {
        let input = &many[..];
        let parsed = parse_html(b"<body>", false, DepthLimit::UNLIMITED).expect("parses");
        // SAFETY: `parsed` owns the document; only this test touches it.
        let ld = unsafe { parsed.raw_doc().as_doc() };
        let mut ac = crate::lexbor::adapter::arena_copy::copy_document(ld).expect("copy");
        // SAFETY: as above.
        let mut lm = unsafe { LexMut::assume_exclusive(ld) };
        let mut am = arena::HtmlDocMut::new(&mut ac);
        let bi = lex_walk(ld)
            .iter()
            .position(|n| n.element().is_some_and(|e| e.local_name() == b"body"))
            .expect("body");
        let (lb, ab) = at(ld, am.read(), bi);
        let (mut lp, mut ap) = (lex::RawNode::from(lb), arena::RawNode::from(ab));
        for name in chain {
            let l = lm.create_element(name).expect("el");
            let a = am.create_element(name).expect("el");
            lm.place(lp, l, lex::Place::Child);
            am.place(ap, a, arena::Place::Child);
            (lp, ap) = (l, a);
        }
        // SAFETY: the context element is live for the call.
        let l = unsafe {
            lf::TransientFragment::parse(
                input,
                true,
                &lf::FragmentContext::Element(lp),
                DepthLimit::DEFAULT,
            )
        };
        let actx = af::FragmentContext::element(am.read(), ap).expect("context");
        let a = af::TransientFragment::parse(input, true, &actx, DepthLimit::DEFAULT);
        assert_eq!(l.err(), a.err(), "{chain:?}");
    }

    /* A context element in a custom namespace, from another document,
     * against the Lexbor DOM taking the same route. */
    for input in [&b"<a>x</a><b/>"[..], b"<svg><g/></svg><c>"] {
        let lother = crate::lexbor::adapter::post_parse::empty_html_document(0).expect("doc");
        // SAFETY: `lother` owns the document; only this test touches it.
        let lo = unsafe { lother.raw_doc().as_doc() };
        let mut lom = unsafe { LexMut::assume_exclusive(lo) };
        let lel = lom.create_element_ns(b"x", b"urn:Custom", b"").expect("el");
        let ltarget = crate::lexbor::adapter::post_parse::empty_html_document(0).expect("doc");
        // SAFETY: as above.
        let lt = unsafe { ltarget.raw_doc().as_doc() };
        let lat = lf::FragmentTag::resolve(lom.node(lel).element().expect("el"), lt)
            .expect("resolve")
            .expect("tag");
        // SAFETY: the target document is live for the call.
        let lfrag = unsafe {
            lf::TransientFragment::parse(
                input,
                true,
                &lf::FragmentContext::Tag {
                    doc: ltarget.raw_doc(),
                    at: lat,
                },
                DepthLimit::DEFAULT,
            )
        }
        .expect("parses");
        let mut ltm = unsafe { LexMut::assume_exclusive(lt) };
        let linto = ltm.create_fragment().expect("frag");
        lfrag.import_into_doc(&mut ltm, linto).expect("import");

        let mut other = crate::html::dom::Document::new(0).expect("doc");
        let mut om = arena::HtmlDocMut::new(&mut other);
        let el = om.create_element_ns(b"x", b"urn:Custom", b"").expect("el");
        let mut target = crate::lexbor::arena_parse::empty_html_document(0).expect("doc");
        let at =
            af::FragmentTag::resolve(om.read().node(el).element().expect("element"), target.doc())
                .expect("resolve")
                .expect("a tag");
        let frag = af::TransientFragment::parse(
            input,
            true,
            &af::FragmentContext::tag(at),
            DepthLimit::DEFAULT,
        )
        .expect("parses");
        let mut tm = target.edit();
        let into = tm.create_fragment().expect("frag");
        frag.import_into_doc(&mut tm, into).expect("import");
        let r = tm.read();
        let lnodes: Vec<_> = ltm
            .node(linto)
            .subtree_with_contents()
            .map(|n| (s(Some(n.node_name())), s(n.ns_uri())))
            .collect();
        let anodes: Vec<_> = r
            .node(into)
            .subtree_with_contents()
            .map(|n| (s(Some(n.node_name())), s(n.ns_uri())))
            .collect();
        assert_eq!(lnodes, anodes, "{}", String::from_utf8_lossy(input));
    }
}
