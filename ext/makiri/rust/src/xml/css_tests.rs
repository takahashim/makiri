//! The CSS matcher over the XML arena (`xml::css`), held to itself and to
//! fixed answers. Its entry points must agree - `select_all` from the
//! document node (an index answers a type selector there) against a walk
//! from the root element, `select_first` against the first of `select_all`,
//! and `matches_any` on every element - over random documents and selectors;
//! the fixed selectors keep the answers the matcher and the CSS-to-XPath
//! lowering it replaced agreed on, while both were built; and what the
//! lowering never answered is held to Chromium, Firefox and WebKit.
#![allow(clippy::disallowed_methods)] /* test inputs */
#![forbid(unsafe_code)]

use crate::css_match::dom::CssNode;
use crate::css_match::{matches_any, select_all, select_first, Namespaces, Scratch};
use crate::xml::css::{XmlHost, XmlNode};
use crate::xml::model::{Document, NodeId};
use crate::xml::tree::parse as xml_parse;

const NS_S: &[u8] = b"urn:s";
const BINDINGS: &[(&[u8], &[u8])] = &[(b"s", NS_S)];

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
/// one document of each kind, with the number of elements found - the
/// answers the matcher and the lowering agreed on.
#[test]
fn the_matcher_keeps_its_answers_on_fixed_selectors() {
    let plain = r#"<r><a id="x" class="x y" k="12"><b/><?pi d?><c k="x-y">t</c></a><a/><p><![CDATA[]]></p><B k="X"/><!--c--><b class="xy"><c/></b></r>"#;
    let spaced =
        r#"<r xmlns:s="urn:s"><s:a s:k="1" k="2"><a/><s:b/></s:a><b xmlns="urn:s"><c/></b></r>"#;
    let mut scratch = Scratch::new();
    let doc = xml_parse(plain.as_bytes()).expect("parses");
    for (sel, want) in [
        ("a", 2),
        ("B", 1),
        ("b", 2),
        ("*", 9),
        ("a b", 1),
        ("a > b", 1),
        ("b + c", 1),
        ("b ~ c", 1),
        ("r > *", 5),
        (".x", 1),
        (".y", 1),
        (".xy", 1),
        ("#x", 1),
        ("[k]", 3),
        ("[k='12']", 1),
        ("[k^='1']", 1),
        ("[k$='2']", 1),
        ("[k*='2']", 1),
        ("[k|=x]", 1),
        ("[k~=x]", 0),
        ("[class~=y]", 1),
        (":first-child", 4),
        (":last-child", 4),
        (":only-child", 2),
        (":first-of-type", 8),
        (":nth-child(2)", 2),
        (":nth-last-child(odd)", 6),
        (":nth-of-type(2)", 1),
        (":root", 1),
        (":not(a)", 7),
        (":is(a, b)", 4),
        (":where(c)", 2),
        ("a:has(c)", 1),
        ("a:has(> b)", 1),
        ("b:has(+ c)", 1),
        (":has(~ c)", 1),
        ("a, b", 4),
        ("[|k]", 3),
        ("*|a", 2),
        ("|a", 2),
        /* A class name holding whitespace, or an empty `~=` token,
         * represents nothing. */
        (r".x\ y", 0),
        ("[class~='']", 0),
        /* `:root` is the document element, not any parentless element. */
        ("a:root", 0),
        /* A negative step: only the leading positions. */
        ("r > :nth-child(-n+3)", 3),
        ("r > :nth-child(-2n+2)", 1),
        ("r > :nth-child(-2n+1)", 1),
        ("r > :nth-child(-n+0)", 0),
    ] {
        let found = matched(&doc, sel, &mut scratch).map(|v| v.len());
        assert_eq!(found, Some(want), "{sel:?}");
    }
    let doc = xml_parse(spaced.as_bytes()).expect("parses");
    for (sel, want) in [
        ("s|a", 1),
        ("|a", 1),
        ("*|a", 2),
        ("s|*", 4),
        ("|*", 2),
        ("*|*", 6),
        ("s|a > |a", 1),
        ("s|b", 2),
        ("*|b > *|c", 1),
        ("[s|k]", 1),
        ("[s|k='1']", 1),
        ("[k]", 1),
        (":not(s|a)", 5),
        ("*|*:first-child", 4),
    ] {
        let found = matched(&doc, sel, &mut scratch).map(|v| v.len());
        assert_eq!(found, Some(want), "{sel:?}");
    }
}

/// Random documents and selectors, each asked of every entry point;
/// `MAKIRI_XML_CSS_SEED` picks another run.
#[test]
fn the_entry_points_agree_on_random_selectors() {
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
            if let Some(all) = matched(&doc, &sel, &mut scratch) {
                compared += 1;
                found += usize::from(!all.is_empty());
            }
        }
    }
    /* An empty answer checks little: most must find something. */
    assert!(
        compared > 3000 && found > compared / 3,
        "{compared} selectors compared, {found} finding something (seed {seed:#x})"
    );
}

/// What the lowering never answered, held to what Chromium, Firefox and
/// WebKit answer over the same XML document
/// (`spec/conformance/css_browser_cases.rb`'s `xml-xhtml` and `xml-plain`).
/// An XHTML element in an XML document has the form states, and compares
/// its names and values as written; `:read-write` (M9) and `:empty` (M10)
/// are the browsers'.
#[test]
fn the_matcher_answers_as_the_browsers_do() {
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
