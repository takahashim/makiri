//! The CSS matcher over the XML arena (`xml::css`) held to the lowering it is
//! to replace: `css::compile_owned` turns a selector into XPath, which the
//! XPath engine evaluates over the same document. Two implementations that
//! share only the selector parser, so where both answer they must answer
//! alike.
//!
//! Only what the two mean alike is compared. Both are asked from the
//! document node, where the lowering's scope (descendants of the context)
//! and the matcher's (the whole tree, descendants returned) coincide. A
//! bare type selector is the open question - the lowering reads it as no
//! namespace, the matcher as any - so the documents that hold namespaced
//! elements are queried with prefixed type selectors only. What the
//! lowering refuses (the form states, `i`, `of S`) it does not answer, and
//! the comparison skips.

#![forbid(unsafe_code)]

use crate::css::{compile_owned, CssNs, Form};
use crate::css_match::dom::CssNode;
use crate::css_match::{matches_any, select_all, select_first, Namespaces, Scratch};
use crate::text::VerifiedText;
use crate::xml::css::{XmlHost, XmlNode};
use crate::xml::model::{Document, NodeId};
use crate::xml::tree::parse as xml_parse;
use crate::xpath::ctx::XPathValue;
use crate::xpath::limits::Budget;

const NS_S: &[u8] = b"urn:s";
const BINDINGS: &[(&[u8], &[u8])] = &[(b"s", NS_S)];

/// The lowering's answer from the document node, or `None` where it refuses
/// the selector or its evaluation fails.
fn lowered(doc: &Document, selector: &str) -> Option<Vec<NodeId>> {
    let ctx = crate::xml::xpath::context(doc, doc.doc_node());
    ctx.register_ns(b"s", NS_S).ok()?;
    let source = VerifiedText::from_bytes(selector.as_bytes())?;
    let mut budget = Budget::with_limits(ctx.limits());
    let ns = CssNs {
        default_namespace: false,
    };
    let ast = compile_owned(source, &ns, Form::Select, &mut budget).ok()?;
    match ctx.evaluate(&ast, None).ok()? {
        XPathValue::NodeSet(set) => Some(
            set.as_slice()
                .iter()
                .filter_map(|t| NodeId::from_token(t.as_ptr() as usize))
                .collect(),
        ),
        _ => None,
    }
}

/// The matcher's answer from the document node, checked against its own
/// `select_first` and `matches_any`; `None` where it refuses the selector.
fn matched(doc: &Document, selector: &str, scratch: &mut Scratch<XmlHost>) -> Option<Vec<NodeId>> {
    let parsed = crate::selectors::parse(selector.as_bytes()).ok()?;
    let ns = Namespaces::new(BINDINGS, None);
    let root = XmlNode::new(doc, doc.doc_node());
    let all = select_all(scratch, root, parsed.groups(), &ns).ok()?;
    /* From the document node a type selector is answered from an index;
     * from the root element, by a walk. They must agree. */
    if let Some(top) = doc.root().map(|id| XmlNode::new(doc, id)) {
        let mut walked = Vec::new();
        if matches_any(scratch, parsed.groups(), &ns, top).ok()? {
            walked.push(top);
        }
        walked.extend(select_all(scratch, top, parsed.groups(), &ns).ok()?);
        assert!(
            walked == all,
            "the index and the walk disagree on {selector:?}"
        );
    }
    let first = select_first(scratch, root, parsed.groups(), &ns).ok()?;
    assert!(
        first.map(XmlNode::id) == all.first().map(|n| n.id()),
        "select_first for {selector:?}"
    );
    let mut n = root;
    while let Some(next) = n.preorder_next(root) {
        n = next;
        if n.element().is_some() {
            let one = matches_any(scratch, parsed.groups(), &ns, n).ok()?;
            assert_eq!(one, all.contains(&n), "matches_any for {selector:?}");
        }
    }
    Some(all.into_iter().map(XmlNode::id).collect())
}

/// Compare the two on `selector`: `None` when either refused it, else how
/// many elements both found.
fn agree(
    doc: &Document,
    src: &str,
    selector: &str,
    scratch: &mut Scratch<XmlHost>,
) -> Option<usize> {
    let new = matched(doc, selector, scratch)?;
    /* The lowering's `:empty` is Lexbor's, which counts a processing
     * instruction and an empty CDATA section; the matcher's is the
     * browsers' (M10), checked against them below. */
    if selector.contains(":empty") {
        return Some(new.len());
    }
    let old = lowered(doc, selector)?;
    assert_eq!(
        new, old,
        "the matcher and the lowering disagree on {selector:?} over\n{src}"
    );
    Some(new.len())
}

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len())]
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

const NAMES: &[&str] = &["a", "b", "c", "p", "B"];
const VALUES: &[&str] = &["x", "y", "x y", "xy", "", "1", "12", "x-y", "X"];

/// A document of up to `budget` elements: names from [`NAMES`], prefixed
/// `s:` (bound to [`NS_S`]) when `namespaced`, with `id`, `class`, `k` and
/// (namespaced) `s:k` attributes, and text, comments, processing
/// instructions and CDATA sections between them.
fn document(rng: &mut Rng, namespaced: bool) -> String {
    let mut out = String::from(if namespaced {
        r#"<r xmlns:s="urn:s">"#
    } else {
        "<r>"
    });
    let mut open: Vec<String> = Vec::new();
    for _ in 0..rng.below(40) + 5 {
        match rng.below(10) {
            0 => out.push('t'),
            1 => out.push_str("<!--c-->"),
            2 => out.push_str("<?pi d?>"),
            3 => out.push_str("<![CDATA[]]>"),
            4 | 5 if !open.is_empty() => {
                let name = open.pop().unwrap_or_default();
                out.push_str(&format!("</{name}>"));
            }
            _ => {
                let local = rng.pick(NAMES);
                let name = if namespaced && rng.chance(40) {
                    format!("s:{local}")
                } else {
                    local.to_string()
                };
                out.push('<');
                out.push_str(&name);
                for attr in ["id", "class", "k", "s:k"] {
                    if (attr != "s:k" || namespaced) && rng.chance(30) {
                        out.push_str(&format!(r#" {attr}="{}""#, rng.pick(VALUES)));
                    }
                }
                if rng.chance(30) {
                    out.push_str("/>");
                } else {
                    out.push('>');
                    open.push(name);
                }
            }
        }
    }
    while let Some(name) = open.pop() {
        out.push_str(&format!("</{name}>"));
    }
    out.push_str("</r>");
    out
}

/// A compound: a type or universal selector, then a few simple selectors.
fn compound(rng: &mut Rng, namespaced: bool, depth: usize) -> String {
    let mut out = String::new();
    match rng.below(4) {
        0 => out.push('*'),
        1 if namespaced => out.push_str(rng.pick(&["*|*", "s|*", "|*"])),
        _ => {
            let local = rng.pick(NAMES);
            if namespaced {
                out.push_str(rng.pick(&["s|", "|", "*|"]));
            }
            out.push_str(local);
        }
    }
    for _ in 0..rng.below(3) {
        out.push_str(&simple(rng, namespaced, depth));
    }
    out
}

fn simple(rng: &mut Rng, namespaced: bool, depth: usize) -> String {
    let value = rng.pick(VALUES);
    match rng.below(14) {
        0 => format!(".{}", rng.pick(&["x", "y", "xy", "X"])),
        1 => format!("#{}", rng.pick(&["x", "y", "xy", "X", "1"])),
        2 => {
            let name = if namespaced && rng.chance(30) {
                "s|k"
            } else {
                rng.pick(&["k", "id", "class", "|k"])
            };
            let op = rng.pick(&["", "=", "~=", "|=", "^=", "$=", "*="]);
            if op.is_empty() {
                format!("[{name}]")
            } else {
                format!(r#"[{name}{op}"{value}"]"#)
            }
        }
        3 => rng
            .pick(&[
                ":first-child",
                ":last-child",
                ":only-child",
                ":first-of-type",
                ":last-of-type",
                ":only-of-type",
                ":empty",
                ":root",
            ])
            .to_string(),
        4 => format!(
            ":{}({})",
            rng.pick(&[
                "nth-child",
                "nth-last-child",
                "nth-of-type",
                "nth-last-of-type"
            ]),
            rng.pick(&["1", "2", "odd", "even", "2n+1", "-n+2", "3n"])
        ),
        5..=7 if depth < 2 => {
            let pseudo = rng.pick(&["not", "is", "where", "has"]);
            let mut args = Vec::new();
            for _ in 0..rng.below(2) + 1 {
                let lead = if pseudo == "has" {
                    rng.pick(&["", "> ", "+ ", "~ "])
                } else {
                    ""
                };
                args.push(format!("{lead}{}", chain(rng, namespaced, depth + 1)));
            }
            format!(":{pseudo}({})", args.join(", "))
        }
        _ => String::new(),
    }
}

fn chain(rng: &mut Rng, namespaced: bool, depth: usize) -> String {
    let mut out = compound(rng, namespaced, depth);
    for _ in 0..rng.below(3) {
        out.push_str(rng.pick(&[" ", " > ", " + ", " ~ "]));
        out.push_str(&compound(rng, namespaced, depth));
    }
    out
}

fn selector(rng: &mut Rng, namespaced: bool) -> String {
    let mut out = chain(rng, namespaced, 0);
    if rng.chance(20) {
        out.push_str(", ");
        out.push_str(&chain(rng, namespaced, 0));
    }
    out
}

/// Fixed shapes first: each kind of simple selector and combinator, over
/// one document of each kind.
#[test]
fn the_matcher_agrees_with_the_lowering_on_fixed_selectors() {
    let plain = r#"<r><a id="x" class="x y" k="12"><b/><?pi d?><c k="x-y">t</c></a><a/><p><![CDATA[]]></p><B k="X"/><!--c--><b class="xy"><c/></b></r>"#;
    let spaced =
        r#"<r xmlns:s="urn:s"><s:a s:k="1" k="2"><a/><s:b/></s:a><b xmlns="urn:s"><c/></b></r>"#;
    let mut scratch = Scratch::new();
    let doc = xml_parse(plain.as_bytes()).expect("parses");
    for sel in [
        "a",
        "B",
        "b",
        "*",
        "a b",
        "a > b",
        "b + c",
        "b ~ c",
        "r > *",
        ".x",
        ".y",
        ".xy",
        "#x",
        "[k]",
        "[k='12']",
        "[k^='1']",
        "[k$='2']",
        "[k*='2']",
        "[k|=x]",
        "[k~=x]",
        "[class~=y]",
        ":first-child",
        ":last-child",
        ":only-child",
        ":first-of-type",
        ":nth-child(2)",
        ":nth-last-child(odd)",
        ":nth-of-type(2)",
        ":empty",
        ":root",
        ":not(a)",
        ":is(a, b)",
        ":where(c)",
        "a:has(c)",
        "a:has(> b)",
        "b:has(+ c)",
        ":has(~ c)",
        "a, b",
        "[|k]",
        "*|a",
        "|a",
    ] {
        assert!(
            agree(&doc, plain, sel, &mut scratch).is_some(),
            "{sel:?} answered"
        );
    }
    let doc = xml_parse(spaced.as_bytes()).expect("parses");
    for sel in [
        "s|a",
        "|a",
        "*|a",
        "s|*",
        "|*",
        "*|*",
        "s|a > |a",
        "s|b",
        "*|b > *|c",
        "[s|k]",
        "[s|k='1']",
        "[k]",
        ":not(s|a)",
        "*|*:first-child",
    ] {
        assert!(
            agree(&doc, spaced, sel, &mut scratch).is_some(),
            "{sel:?} answered"
        );
    }
}

/// Random documents and selectors; `MAKIRI_XML_CSS_SEED` picks another run.
#[test]
fn the_matcher_agrees_with_the_lowering_on_random_selectors() {
    let seed = std::env::var("MAKIRI_XML_CSS_SEED")
        .ok()
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x5EED_C55A_11CE);
    let mut rng = Rng(seed | 1);
    let mut scratch = Scratch::new();
    let (mut compared, mut found) = (0, 0);
    for d in 0..120 {
        let namespaced = d % 2 == 1;
        let src = document(&mut rng, namespaced);
        let Ok(doc) = xml_parse(src.as_bytes()) else {
            continue;
        };
        for _ in 0..40 {
            let sel = selector(&mut rng, namespaced);
            if let Some(n) = agree(&doc, &src, &sel, &mut scratch) {
                compared += 1;
                found += usize::from(n > 0);
            }
        }
    }
    /* A comparison of two empty answers proves little: most must find
     * something. */
    assert!(
        compared > 3000 && found > compared / 3,
        "{compared} selectors compared, {found} finding something (seed {seed:#x})"
    );
}

/// What the lowering refuses and the matcher answers, held to what
/// Chromium, Firefox and WebKit answer over the same XML document
/// (`spec/conformance/css_browser_cases.rb`'s `xml-xhtml` and `xml-plain`).
/// An XHTML element in an XML document has the form states, and compares
/// its names and values as written; `:read-write` (M9) and `:empty` (M10)
/// are the browsers'.
#[test]
fn the_matcher_answers_what_the_lowering_refuses_as_the_browsers_do() {
    let xhtml = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:svg="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><body><form><input id="i1" type="checkbox" checked=""/><input id="i2" type="TEXT" required="" placeholder="p"/><input id="i3" disabled=""/><fieldset disabled=""><input id="i4"/></fieldset><select><option selected="">a</option></select></form><a id="a1" href="x">a</a><a id="a2">b</a><DIV id="D"/><P title="Hello World"/><svg:svg><svg:a xlink:href="x"><svg:path/></svg:a><svg:a href="y"/></svg:svg></body></html>"#;
    let plain = r#"<r><p class="x" title="Hello World"/><p/><q class="x"/><p class="x"/><e><?pi x?></e><e><![CDATA[]]></e><e><!--c--></e><e>t</e></r>"#;
    let doc = xml_parse(xhtml.as_bytes()).expect("parses");
    let mut scratch = Scratch::new();
    let count = |doc: &Document, sel: &str, scratch: &mut Scratch<XmlHost>| {
        matched(doc, sel, scratch).map(|v| v.len())
    };
    for (sel, want) in [
        (":checked", 2),
        (":disabled", 3),
        (":enabled", 4),
        (":required", 1),
        (":optional", 4),
        (":placeholder-shown", 1),
        (":link", 3),
        (":any-link", 3),
        (":read-write", 1),
        ("DIV", 1),
        ("div", 0),
        ("INPUT", 0),
        ("[type=text]", 0),
        ("[type=TEXT]", 1),
        ("[title='hello world' i]", 1),
        ("[title='hello world']", 0),
    ] {
        assert_eq!(count(&doc, sel, &mut scratch), Some(want), "{sel:?}");
    }
    let doc = xml_parse(plain.as_bytes()).expect("parses");
    for (sel, want) in [
        (":nth-child(2n+1 of .x)", 2),
        ("e:empty", 3),
        (":nth-last-child(1 of p)", 1),
        (":checked", 0),
        (":disabled", 0),
        (":link", 0),
    ] {
        assert_eq!(count(&doc, sel, &mut scratch), Some(want), "{sel:?}");
    }
}
