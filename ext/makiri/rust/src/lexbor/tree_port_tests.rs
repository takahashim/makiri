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
use crate::lexbor::adapter::arena_copy::copy_document;
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
    let mut out = String::new();
    let _ = writeln!(out, "compat {}", d.compat_mode);
    let mut stack: Vec<(NodeId, usize)> = vec![(NodeId::DOCUMENT, 0)];
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
    out
}

/// Lexbor's tree, copied into the arena.
pub fn lexbor_dump(input: &[u8], offsets: bool) -> Option<String> {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).ok()?;
    let doc = copy_document(parsed.doc()).expect("the copy");
    Some(dump(&doc, offsets))
}

/// The port's tree.
pub fn port_dump(input: &[u8], offsets: bool) -> Option<String> {
    let input = crate::utf8_input::sanitize(input, false)?;
    let parsed = crate::html::tree::parse_document(input.as_slice()).ok()?;
    Some(dump(&parsed.doc, offsets))
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
    if let Err(e) = check(input.as_bytes(), false) {
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
        if let Err(e) = check(&t.data, false) {
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
        if let Err(e) = check(input.as_bytes(), false) {
            failures.push(format!("seed {seed}: {input:?}\n{e}"));
            if failures.len() >= 5 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
