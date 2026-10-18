//! The ported tree builder against Lexbor's: `lexbor::tree_diff`'s dumps of
//! both parses must be equal, on hand-picked, WPT and random inputs, for
//! documents and fragments, with the parse guard's verdicts alike.

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use crate::conformance::dat::{self, Scripting};
use crate::conformance::tree_construction_dir;
use crate::lexbor::adapter::arena_copy::copy_document;
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::adapter::tree_guard::DepthLimit;
use crate::lexbor::tree_diff::*;

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

/// Lexbor's parse (its tree, then the copy into the arena) and the port's,
/// timed on a bench-shaped document. A measurement, not a check:
/// `cargo test --release ... tree_port_speed -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn tree_port_speed() {
    use std::time::Instant;
    let mut rows = String::new();
    for i in 1..=20_000 {
        rows.push_str(&format!(
            "<li class=\"item r{}\" data-id=\"{i}\"><a href=\"/p/{i}\" class=\"link\">item {i}</a><span class=\"meta\">tag{} &amp; more</span></li>\n",
            i % 7,
            i % 13
        ));
    }
    let input = format!(
        "<!doctype html><html><head><title>bench</title></head><body><main><ul>{rows}</ul></main><table><tr><td>a<b>b</td></tr></table></body></html>"
    );
    let input = input.as_bytes();
    let best = |f: &dyn Fn()| {
        let mut b = f64::MAX;
        for _ in 0..10 {
            let t = Instant::now();
            f();
            b = b.min(t.elapsed().as_secs_f64());
        }
        b
    };
    let lexbor_tree = best(&|| {
        std::hint::black_box(parse_html(input, false, DepthLimit::DEFAULT).unwrap());
    });
    let lexbor_copy = best(&|| {
        let p = parse_html(input, false, DepthLimit::DEFAULT).unwrap();
        std::hint::black_box(copy_document(p.doc()).unwrap());
    });
    let port = best(&|| {
        std::hint::black_box(
            crate::html::tree::parse_html(input, false, DepthLimit::DEFAULT).unwrap(),
        );
    });
    let mb = input.len() as f64 / 1e6;
    eprintln!(
        "{mb:.1} MB: lexbor {:.1} ms, lexbor+copy {:.1} ms, port {:.1} ms; port/lexbor {:.2}, port/(lexbor+copy) {:.2}",
        lexbor_tree * 1e3,
        lexbor_copy * 1e3,
        port * 1e3,
        port / lexbor_tree,
        port / lexbor_copy
    );
}

/// The two parsers over a directory of real pages - the web100k corpus
/// (github.com/EmilStenstrom/web100k), decompressed to `*.html` files in
/// `WEB100K_DIR`. Every page must give the same tree, offsets included, with
/// no limit, and the same verdict under the default guard. Pages that differ
/// are copied to `WEB100K_OUT` (when set) for a repro, and the run fails
/// naming the first few.
#[test]
#[ignore = "needs the web100k corpus in WEB100K_DIR"]
fn the_port_builds_lexbors_tree_on_web100k() {
    let Some(dir) = std::env::var_os("WEB100K_DIR") else {
        panic!("set WEB100K_DIR to a directory of decompressed web100k pages");
    };
    let out = std::env::var_os("WEB100K_OUT").map(std::path::PathBuf::from);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("WEB100K_DIR is a directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    paths.sort();
    let (mut n, mut bytes, mut differ, mut verdicts) = (0usize, 0usize, Vec::new(), Vec::new());
    for p in &paths {
        let input = std::fs::read(p).expect("a page");
        n += 1;
        bytes += input.len();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let mut keep = false;
        if let Err(e) = check(&input, true) {
            differ.push(format!("{name}\n{e}"));
            keep = true;
        }
        let l = parse_html(&input, false, DepthLimit::DEFAULT).map(|_| ());
        let r = crate::html::tree::parse_html(&input, false, DepthLimit::DEFAULT).map(|_| ());
        if l != r {
            verdicts.push(format!("{name}: lexbor {l:?}, port {r:?}"));
            keep = true;
        }
        if let (true, Some(o)) = (keep, &out) {
            let _ = std::fs::create_dir_all(o);
            let _ = std::fs::write(o.join(&name), &input);
        }
    }
    eprintln!(
        "web100k: {n} pages, {:.1} MB; {} trees differ, {} verdicts differ",
        bytes as f64 / 1e6,
        differ.len(),
        verdicts.len()
    );
    assert!(
        differ.is_empty() && verdicts.is_empty(),
        "{}\n\n{}",
        verdicts.join("\n"),
        differ
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

/// One WPT test through the port, as `conformance_tests::run` runs it
/// through Lexbor: `None` for a scripting-on test.
fn port_run(t: &dat::DatTest) -> Option<String> {
    use crate::conformance::tree::{self, Quirks, Tree};
    use crate::html::dom::conformance::{read_children, read_document};
    use crate::html::tree::guard::ContextOptions;
    if t.scripting == Scripting::On {
        return None;
    }
    let tree = match &t.fragment {
        Some(ctx) => {
            let (tag, ns) = context_ids(ctx);
            let is_select = tag == crate::html::res::names::TAG_SELECT
                && ns == crate::html::res::names::NS_HTML;
            let options = ContextOptions {
                fresh: is_select,
                after_optgroup: is_select,
            };
            let f = crate::html::tree::parse_fragment(
                &t.data,
                false,
                tag,
                ns,
                options,
                DepthLimit::UNLIMITED,
            )
            .expect("the port parses a fragment");
            Tree {
                quirks: None::<Quirks>,
                children: read_children(&f.doc, f.root),
            }
        }
        None => {
            let p = crate::html::tree::parse_document(&t.data).expect("the port parses");
            read_document(&p.doc)
        }
    };
    Some(tree::html5lib(&tree))
}

/// Every WPT tree-construction test through the port, held to the pinned
/// data: every test passes or is listed for the port in
/// `spec/conformance/parser_known_divergences.txt`. Where the port has fixed a
/// divergence, the entry names Lexbor alone, and this is the test that holds
/// the fix to the data.
#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn wpt_tree_construction_through_the_port() {
    let tests = dat::read_dir(&tree_construction_dir()).expect("WPT data");
    let known = crate::conformance::known::load("port");
    let (mut pass, mut known_hit, mut skipped) = (0, 0, 0);
    let mut failures = Vec::new();
    let mut now_passing = Vec::new();
    for t in &tests {
        let Some(actual) = port_run(t) else {
            skipped += 1;
            continue;
        };
        match (actual == t.document, known.get(&t.key())) {
            (true, None) => pass += 1,
            (true, Some(id)) => now_passing.push(format!("{} ({id})", t.key())),
            (false, Some(_)) => known_hit += 1,
            (false, None) => failures.push(format!(
                "{}{}\n--- input ---\n{}\n--- expected ---\n{}\n--- actual ---\n{}",
                t.key(),
                t.fragment
                    .as_ref()
                    .map(|c| format!(" (context {c})"))
                    .unwrap_or_default(),
                String::from_utf8_lossy(&t.data),
                t.document,
                actual
            )),
        }
    }
    eprintln!(
        "WPT tree construction through the port: {} tests, {pass} pass, {known_hit} known, \
         {skipped} scripting-on skipped, {} fail, {} now passing",
        tests.len(),
        failures.len(),
        now_passing.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(
        now_passing.is_empty(),
        "listed as known for the port but now passing - remove it for the port: {now_passing:?}"
    );
}
