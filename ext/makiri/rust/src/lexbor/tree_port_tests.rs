//! The ported tree builder (`crate::html::tree`) against Lexbor's: the same
//! input parsed by Lexbor and copied into the arena, and parsed by the port
//! straight into the arena, must give the same arena - every node's kind,
//! names as written, namespaces and prefixes, attributes in order with the
//! `id`/`class` shortcuts, character data, template contents, and the
//! document's quirks mode (`docs/rewrite_parser.ja.md`, stage 4).

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::fmt::Write as _;

use crate::conformance::dat::{self, Scripting};
use crate::conformance::tree_construction_dir;
use crate::html::dom::{Document, NodeId, NodeType};
use crate::lexbor::adapter::arena_copy::{copy_children, copy_document};
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::adapter::tree_guard::DepthLimit;

fn text(b: Option<&[u8]>) -> String {
    match b {
        Some(b) => format!("{:?}", String::from_utf8_lossy(b)),
        None => "-".into(),
    }
}

/// Everything the port must reproduce, one line per node, in tree order.
pub fn dump(d: &Document, offsets: bool) -> String {
    let mut out = format!("compat {}\n", d.compat_mode);
    walk(d, &mut out, vec![(NodeId::DOCUMENT, 0)], offsets);
    out
}

/// [`dump`] of the children of `parent` alone.
pub fn dump_children(d: &Document, parent: NodeId, offsets: bool) -> String {
    let mut out = String::new();
    let mut roots = Vec::new();
    let mut c = d.first_child(parent);
    while let Some(x) = c {
        roots.push((x, 0));
        c = d.next(x);
    }
    roots.reverse();
    walk(d, &mut out, roots, offsets);
    out
}

fn walk(d: &Document, out: &mut String, mut stack: Vec<(NodeId, usize)>, offsets: bool) {
    while let Some((n, depth)) = stack.pop() {
        let pad = "  ".repeat(depth);
        let kind = d.kind(n);
        let name = match kind {
            NodeType::Element => text(d.names.tag_name(d.local(n) as usize)),
            _ => d.local(n).to_string(),
        };
        let _ = write!(
            out,
            "{pad}{kind:?} {name} ns={} prefix={}",
            text(d.ns_uri(d.ns(n))),
            text(d.prefix_text(d.prefix(n)))
        );
        match kind {
            NodeType::Element => {
                let _ = write!(out, " written={}", text(d.element_written(n)));
                let mut a = d.first_attr(n);
                let mut i = 0;
                while let Some(x) = a {
                    let _ = write!(
                        out,
                        "\n{pad}  @{} ns={} own={} prefix={} written={} = {}{}{}",
                        text(d.names.attr_name(d.local(x) as usize)),
                        text(d.ns_uri(d.ns(x))),
                        text(d.ns_uri(d.attr_own_ns(x))),
                        text(d.prefix_text(d.prefix(x))),
                        text(d.attr_written(x)),
                        text(d.attr_value(x)),
                        if d.id_attr(n) == Some(x) { " [id]" } else { "" },
                        if d.class_attr(n) == Some(x) {
                            " [class]"
                        } else {
                            ""
                        },
                    );
                    a = d.next_attr(x);
                    i += 1;
                }
                let _ = i;
            }
            NodeType::Pi => {
                let _ = write!(out, " {} {}", text(d.pi_target(n)), text(d.data(n)));
            }
            NodeType::DocumentType => {
                if let Some((a, b, c)) = d.doctype(n) {
                    let _ = write!(
                        out,
                        " {} {} {}",
                        text(Some(a)),
                        text(Some(b)),
                        text(Some(c))
                    );
                }
            }
            _ => {
                if let Some(data) = d.data(n) {
                    let _ = write!(out, " {}", text(Some(data)));
                }
            }
        }
        if offsets {
            if let Some(o) = d.source_offset(n) {
                let _ = write!(out, " @{o}");
            }
        }
        out.push('\n');
        let mut children = Vec::new();
        if let Some(c) = d.template_content(n) {
            children.push((c, depth + 1));
        }
        let mut c = d.first_child(n);
        while let Some(x) = c {
            children.push((x, depth + 1));
            c = d.next(x);
        }
        stack.extend(children.into_iter().rev());
    }
}

/// Lexbor's tree, copied into the arena.
pub fn lexbor_dump(input: &[u8], offsets: bool) -> Option<String> {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).ok()?;
    let doc = copy_document(parsed.doc()).expect("the copy");
    Some(dump(&doc, offsets))
}

/// The port's tree.
pub fn port_dump(input: &[u8], offsets: bool) -> Option<String> {
    let parsed = crate::html::tree::parse_html(input, false, DepthLimit::UNLIMITED).ok()?;
    Some(dump(parsed.arena(), offsets))
}

/// The first line where two dumps differ, with some context.
fn first_difference(a: &str, b: &str) -> String {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let i = la
        .iter()
        .zip(&lb)
        .position(|(x, y)| x != y)
        .unwrap_or(la.len().min(lb.len()));
    let from = i.saturating_sub(3);
    format!(
        "line {i}\n--- lexbor\n{}\n--- port\n{}",
        la[from..(i + 3).min(la.len())].join("\n"),
        lb[from..(i + 3).min(lb.len())].join("\n")
    )
}

/// Whether the port builds Lexbor's tree for `input`; the difference when
/// not.
pub fn check(input: &[u8], offsets: bool) -> Result<(), String> {
    let l = lexbor_dump(input, offsets);
    let p = port_dump(input, offsets);
    match (l, p) {
        (Some(l), Some(p)) if l == p => Ok(()),
        (Some(l), Some(p)) => Err(first_difference(&l, &p)),
        (l, p) => Err(format!(
            "lexbor parsed: {}, port parsed: {}",
            l.is_some(),
            p.is_some()
        )),
    }
}

#[track_caller]
fn same(input: &str) {
    if let Err(e) = check(input.as_bytes(), true) {
        panic!("{input:?}\n{e}");
    }
}

#[test]
fn the_port_builds_lexbors_tree() {
    for input in [
        "",
        "x",
        "<!DOCTYPE html><title>t</title><p>a<b>b<i>c</p>d</i>e",
        "<html lang=en><head><meta charset=utf-8></head><body class=x id=y>t</body></html>",
        "<table><tr><td>1<td>2</table>",
        "<table>x<tr>y</table>",
        "<a><p><a>x</a></p>",
        "<b><b><b><b>x</b></b></b></b>",
        "<p><b x><b x=\"\"><b x><b x><b x>t",
        "<svg viewbox='0 0 1 1'><foreignObject><p>x</p></foreignObject><path d=1/></svg>",
        "<math definitionurl=x><mi>t</mi><annotation-xml encoding=text/html><div>d</div></annotation-xml></math>",
        "<svg xlink:href=a xml:lang=b xmlns=c xmlns:xlink=d></svg>",
        "<template><tr><td>x</td></tr></template>",
        "<select><option>a<option>b<optgroup><option>c</select>",
        "<frameset><frame></frameset>",
        "<pre>\nx</pre><textarea>\ny</textarea>",
        "<!-- c --><?php echo 1 ?><!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\">",
        "<div id=a id=b class=c>",
        "<ruby>a<rb>b<rt>c<rp>d</ruby>",
        "<form><form></form><input type=hidden><input type=HIDDEN>",
        "<table><input type=hidden><input></table>",
        "<html><body></body></html><!-- after -->x",
        "<p>a\0b</p><svg>\0</svg>",
        "<plaintext><p>",
        "<h1><h2>x</h1></h2>",
        "<li><li><dd><dt>",
        "<button><button>",
        "<image src=x>",
        "</br></p>",
        "<nobr><nobr>x",
        "<math xmlns=y xmlns=y z><svg xmlns=a xlink:href=b xmlns=c xlink:href=d e>",
    ] {
        same(input);
    }
}

#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn the_port_builds_lexbors_tree_on_wpt_inputs() {
    let tests = dat::read_dir(&tree_construction_dir()).expect("WPT data");
    let mut failures = Vec::new();
    let mut n = 0;
    for t in tests
        .iter()
        .filter(|t| t.fragment.is_none() && t.scripting != Scripting::On)
    {
        n += 1;
        if let Err(e) = check(&t.data, true) {
            failures.push(format!(
                "{} {:?}\n{e}",
                t.key(),
                String::from_utf8_lossy(&t.data)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {n} differ:\n\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn the_dump_sees_the_tree() {
    let d = port_dump(b"<p class=a>x<svg viewbox=1>", false).unwrap();
    assert!(d.contains("@\"class\""), "{d}");
    assert!(d.contains("written=\"viewBox\""), "{d}");
    assert!(check(b"<p>", false).is_ok());
    assert_ne!(port_dump(b"<p>", false), port_dump(b"<b>", false));
}

/// xorshift64*, seeded, so a failure names its seed.
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

const TAGS: &[&str] = &[
    "html",
    "head",
    "body",
    "title",
    "meta",
    "link",
    "style",
    "script",
    "noscript",
    "noframes",
    "template",
    "base",
    "p",
    "div",
    "span",
    "a",
    "b",
    "i",
    "u",
    "s",
    "em",
    "strong",
    "font",
    "nobr",
    "big",
    "small",
    "code",
    "tt",
    "strike",
    "table",
    "caption",
    "colgroup",
    "col",
    "tbody",
    "thead",
    "tfoot",
    "tr",
    "td",
    "th",
    "form",
    "input",
    "button",
    "select",
    "option",
    "optgroup",
    "selectedcontent",
    "datalist",
    "hr",
    "textarea",
    "pre",
    "listing",
    "li",
    "ul",
    "ol",
    "dl",
    "dd",
    "dt",
    "h1",
    "h2",
    "h6",
    "frameset",
    "frame",
    "iframe",
    "xmp",
    "plaintext",
    "image",
    "img",
    "br",
    "wbr",
    "area",
    "embed",
    "param",
    "source",
    "track",
    "keygen",
    "applet",
    "marquee",
    "object",
    "ruby",
    "rb",
    "rt",
    "rp",
    "rtc",
    "svg",
    "math",
    "mi",
    "mo",
    "mtext",
    "annotation-xml",
    "foreignObject",
    "desc",
    "path",
    "g",
    "linearGradient",
    "address",
    "section",
    "nav",
    "main",
    "menu",
    "summary",
    "details",
    "dialog",
    "search",
    "center",
    "blockquote",
    "figure",
    "fieldset",
    "legend",
    "label",
    "my-el",
    "x",
    "sarcasm",
    "isindex",
    "math",
    "mglyph",
    "malignmark",
];
const ATTRS: &[&str] = &[
    "id=a",
    "id=b",
    "class=c",
    "selected",
    "selected=''",
    "multiple",
    "size=1",
    "size=4",
    "disabled",
    "type=hidden",
    "type=HIDDEN",
    "type=text",
    "encoding=text/html",
    "encoding=application/xhtml+xml",
    "xlink:href=x",
    "xml:lang=en",
    "xmlns=y",
    "xmlns:xlink=z",
    "definitionurl=u",
    "viewbox='0 0 1 1'",
    "color=red",
    "face=f",
    "x",
    "x=''",
    "x=1",
    "Y=2",
    "data-q=\"&amp;\"",
];
const TEXTS: &[&str] = &[
    "t", " ", "\n", "  x  ", "\0", "a\0b", "&amp;", "&lt;", "&#xFFFD;", "\u{FFFD}", "&notin;",
    "&notit;", "é", "\r\n", "\t",
];
const OTHERS: &[&str] = &[
    "<!-- c -->",
    "<!--",
    "<?pi x?>",
    "<!DOCTYPE html>",
    "<!doctype html public \"-//W3C//DTD HTML 4.0 Transitional//EN\">",
    "<![CDATA[x]]>",
    "</>",
    "<",
    "</",
    "<!",
    "&",
    "<a b c d>",
];

/// A random input from tag, attribute, text and markup fragments.
fn random_input(rng: &mut Rng) -> String {
    let mut s = String::new();
    let n = 1 + rng.below(30);
    for _ in 0..n {
        match rng.below(10) {
            0..=3 => {
                let tag = rng.pick(TAGS);
                s.push('<');
                s.push_str(tag);
                for _ in 0..rng.below(3) {
                    s.push(' ');
                    s.push_str(rng.pick(ATTRS));
                }
                if rng.below(8) == 0 {
                    s.push('/');
                }
                s.push('>');
            }
            4..=6 => {
                s.push_str("</");
                s.push_str(rng.pick(TAGS));
                s.push('>');
            }
            7 | 8 => s.push_str(rng.pick(TEXTS)),
            _ => s.push_str(rng.pick(OTHERS)),
        }
    }
    s
}

#[test]
fn the_port_builds_lexbors_tree_on_random_inputs() {
    let count: u64 = std::env::var("TREE_PORT_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let seed0: u64 = std::env::var("TREE_PORT_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut failures = Vec::new();
    for seed in seed0..seed0 + count {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let input = random_input(&mut rng);
        if let Err(e) = check(input.as_bytes(), true) {
            failures.push(format!("seed {seed}: {input:?}\n{e}"));
            if failures.len() >= 5 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Whether both parsers accept `input` under `limit`, or refuse it the same
/// way.
fn same_verdict(input: &[u8], limit: DepthLimit) {
    let l = parse_html(input, false, limit).map(|_| ());
    let p = crate::html::tree::parse_html(input, false, limit).map(|_| ());
    assert_eq!(l, p, "{:?} under {limit:?}", String::from_utf8_lossy(input));
}

#[test]
fn the_port_guards_as_lexbor_does() {
    use crate::html::tree::guard::MAX_SELECT_OPTIONS;
    for n in [397, 398, 399, 400, 401] {
        let deep = "<div>".repeat(n);
        same_verdict(deep.as_bytes(), DepthLimit::DEFAULT);
        same_verdict(format!("{deep}<br>").as_bytes(), DepthLimit::DEFAULT);
        same_verdict(format!("{deep}<b><i>").as_bytes(), DepthLimit::DEFAULT);
        same_verdict(format!("<table>{deep}").as_bytes(), DepthLimit::DEFAULT);
        same_verdict(format!("<svg>{deep}").as_bytes(), DepthLimit::DEFAULT);
    }
    for d in [0, 1, 2, 3, 5] {
        for input in [
            "",
            "<p>",
            "<p><b>x",
            "<table><tr><td>",
            "<template><div>",
            "<br><img>",
        ] {
            same_verdict(input.as_bytes(), DepthLimit::at_most(d));
        }
    }
    for n in [MAX_SELECT_OPTIONS, MAX_SELECT_OPTIONS + 1] {
        let opts = "<option>".repeat(n);
        same_verdict(format!("<select>{opts}").as_bytes(), DepthLimit::DEFAULT);
        same_verdict(
            format!("<select><optgroup>{opts}").as_bytes(),
            DepthLimit::DEFAULT,
        );
        same_verdict(format!("<datalist>{opts}").as_bytes(), DepthLimit::DEFAULT);
        same_verdict(
            format!("<select>{}", "<option>x</option>".repeat(n)).as_bytes(),
            DepthLimit::DEFAULT,
        );
    }
}

#[test]
fn the_port_stamps_as_lexbor_does() {
    for input in [
        "<p>a<b>b<i>c</p>d</i>e",
        "<table><tr><td>1<td>2</table><br><table>x<br></table>",
        "<html><body><body><p>x",
        "<template><p>a</p><img></template>",
        "\n\n<div>\n<span>x</span>\n</div>",
        "<a><p><a>x</a></p>",
        "<select><option>a<option>b</select>",
    ] {
        if let Err(e) = check(input.as_bytes(), true) {
            panic!("{input:?}\n{e}");
        }
    }
}

#[test]
fn the_empty_document_is_lexbors() {
    for mode in 0..3 {
        let l = crate::lexbor::adapter::post_parse::empty_html_document(mode).unwrap();
        let l = dump(&copy_document(l.doc()).unwrap(), true);
        let p = crate::html::tree::empty_html_document(mode).unwrap();
        assert_eq!(l, dump(p.arena(), true));
    }
}

/* ---------- fragments ---------- */

/// A context for both parsers: `td`, or `svg desc` for a foreign one.
fn context_ids(context: &str) -> (usize, usize) {
    use crate::html::res::names as res;
    let (ns, name) = match context.split_once(' ') {
        Some(("svg", local)) => (res::NS_SVG, local),
        Some(("math", local)) => (res::NS_MATH, local),
        Some((ns, _)) => panic!("unknown fragment context namespace {ns:?}"),
        None => (res::NS_HTML, context),
    };
    /* A name Lexbor's static table lacks stands in as `div`, as
     * `lexbor::arena_fragment` gives it. */
    let tag = crate::html::res::tag_id_by_lower_name(name.to_ascii_lowercase().as_bytes())
        .unwrap_or(res::TAG_DIV);
    (tag, ns)
}

fn lexbor_fragment(
    input: &[u8],
    tag: usize,
    ns: usize,
    limit: DepthLimit,
) -> Result<String, String> {
    use crate::lexbor::adapter::html as lex;
    use crate::lexbor::adapter::tree_guard::OptionContext;
    let at = crate::lexbor::fragment::FragmentTag {
        tag: lex::TagId::from_raw(tag).unwrap(),
        ns: lex::NsId::from_raw(ns),
    };
    let is_select =
        tag == crate::html::res::names::TAG_SELECT && ns == crate::html::res::names::NS_HTML;
    let options = OptionContext::Resolved {
        fresh: is_select,
        after_optgroup: is_select,
    };
    let frag = crate::lexbor::fragment::TransientFragment::parse_standalone(
        input,
        false,
        Some(at),
        options,
        limit,
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut doc = Document::new(0).unwrap();
    let into = doc.create_fragment(None).unwrap();
    frag.with_root(|root| copy_children(&mut doc, root, into, false))
        .unwrap();
    Ok(dump_children(&doc, into, false))
}

fn port_fragment(input: &[u8], tag: usize, ns: usize, limit: DepthLimit) -> Result<String, String> {
    use crate::html::tree::guard::ContextOptions;
    let is_select =
        tag == crate::html::res::names::TAG_SELECT && ns == crate::html::res::names::NS_HTML;
    let options = ContextOptions {
        fresh: is_select,
        after_optgroup: is_select,
    };
    let f = crate::html::tree::parse_fragment(input, false, tag, ns, options, limit)
        .map_err(|e| format!("{e:?}"))?;
    Ok(dump_children(&f.doc, f.root, false))
}

fn check_fragment(input: &[u8], context: &str) -> Result<(), String> {
    let (tag, ns) = context_ids(context);
    let l = lexbor_fragment(input, tag, ns, DepthLimit::UNLIMITED);
    let p = port_fragment(input, tag, ns, DepthLimit::UNLIMITED);
    match (l, p) {
        (Ok(l), Ok(p)) if l == p => Ok(()),
        (Ok(l), Ok(p)) => Err(first_difference(&l, &p)),
        (l, p) => Err(format!("lexbor: {:?}, port: {:?}", l.err(), p.err())),
    }
}

#[test]
fn the_port_parses_fragments_as_lexbor_does() {
    for (input, ctx) in [
        ("<td>x</td><tr>", "tr"),
        ("<td>x", "table"),
        ("x</title>y", "title"),
        ("x</textarea>", "textarea"),
        ("<p>a<b>b", "body"),
        ("<form><form>", "template"),
        ("<form><input>", "form"),
        ("<option>a<option>b", "select"),
        ("<desc><p>x", "svg svg"),
        ("<p>x", "svg desc"),
        ("<mi>x</mi><p>", "math math"),
        ("<plaintext></plaintext>", "div"),
        ("<html><head><body>", "html"),
        ("</p><br>", "p"),
        ("<col><col>", "colgroup"),
        ("<frame>", "frameset"),
        ("x<script>y</script>", "noscript"),
        ("<g><path/></g>", "svg g"),
    ] {
        if let Err(e) = check_fragment(input.as_bytes(), ctx) {
            panic!("{input:?} in {ctx}\n{e}");
        }
    }
}

#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn the_port_parses_fragments_as_lexbor_does_on_wpt_inputs() {
    let tests = dat::read_dir(&tree_construction_dir()).expect("WPT data");
    let mut failures = Vec::new();
    let mut n = 0;
    for t in tests.iter().filter(|t| t.scripting != Scripting::On) {
        let Some(ctx) = &t.fragment else { continue };
        n += 1;
        if let Err(e) = check_fragment(&t.data, ctx) {
            failures.push(format!(
                "{} in {ctx} {:?}\n{e}",
                t.key(),
                String::from_utf8_lossy(&t.data)
            ));
        }
    }
    assert!(n > 100, "{n} fragment tests");
    assert!(
        failures.is_empty(),
        "{} of {n} differ:\n\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn the_port_parses_random_fragments_as_lexbor_does() {
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
    let count: u64 = std::env::var("TREE_PORT_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let mut failures = Vec::new();
    for seed in 1..=count {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let ctx = *rng.pick(CONTEXTS);
        let input = random_input(&mut rng);
        if let Err(e) = check_fragment(input.as_bytes(), ctx) {
            failures.push(format!("seed {seed} in {ctx}: {input:?}\n{e}"));
            if failures.len() >= 5 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn the_port_guards_fragments_as_lexbor_does() {
    use crate::html::tree::guard::MAX_SELECT_OPTIONS;
    for ctx in ["body", "select", "td", "svg svg"] {
        let (tag, ns) = context_ids(ctx);
        for n in [398, 399, 400, 401] {
            let input = "<div>".repeat(n);
            let l = lexbor_fragment(input.as_bytes(), tag, ns, DepthLimit::DEFAULT).err();
            let p = port_fragment(input.as_bytes(), tag, ns, DepthLimit::DEFAULT).err();
            assert_eq!(l.is_some(), p.is_some(), "{n} in {ctx}");
        }
        for n in [MAX_SELECT_OPTIONS, MAX_SELECT_OPTIONS + 1] {
            for input in [
                "<option>".repeat(n),
                format!("<optgroup>{}", "<option>".repeat(n)),
            ] {
                let l = lexbor_fragment(input.as_bytes(), tag, ns, DepthLimit::DEFAULT).err();
                let p = port_fragment(input.as_bytes(), tag, ns, DepthLimit::DEFAULT).err();
                assert_eq!(l.is_some(), p.is_some(), "{n} options in {ctx}");
            }
        }
    }
}
