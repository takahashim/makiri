//! XPath's HTML instance over the arena (`html::dom::xpath`) against the one
//! over Lexbor's DOM (`lexbor::xpath`), as data: the engine is shared, so
//! what differs is each `Dom` - name tests and their case folding, strict and
//! lax namespace matching, the attribute axis, document order, string-values,
//! the element-index `//tag` path. The same Lexbor parse is read by both, the
//! arena through its copy, and an answer is compared by walk position.
//! `lexbor::arena_tests` holds the two to each other over a fixed list of
//! expressions; this generates them ([`expression`]) for the randomized test
//! below and the `html_xpath_diff` fuzz target.

#![allow(
    unsafe_code,
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::collections::HashMap;

use crate::engine_error::{Error, ErrorKind};
use crate::html::dom as arena;
use crate::lexbor::adapter::arena_copy::copy_document;
use crate::lexbor::adapter::html as lex;
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::adapter::tree_guard::DepthLimit;
use crate::token::Token;
use crate::xpath::limits::Budget;
use crate::xpath::parse::parse_owned;
use crate::xpath::value::Val;

/// The prefixes both contexts register.
const NAMESPACES: &[(&[u8], &[u8])] = &[
    (b"svg", b"http://www.w3.org/2000/svg"),
    (b"math", b"http://www.w3.org/1998/Math/MathML"),
    (b"h", b"http://www.w3.org/1999/xhtml"),
    (b"xlink", b"http://www.w3.org/1999/xlink"),
    (b"x", b"urn:x"),
];

/* ---------- the expressions ---------- */

const NAMES: &[&str] = &[
    "html",
    "head",
    "body",
    "title",
    "div",
    "DiV",
    "p",
    "a",
    "b",
    "i",
    "li",
    "td",
    "tr",
    "table",
    "tbody",
    "template",
    "select",
    "option",
    "svg",
    "path",
    "g",
    "math",
    "mi",
    "desc",
    "foreignObject",
    "foreignobject",
    "linearGradient",
    "my-el",
    "x",
    "*",
    "node()",
    "text()",
    "comment()",
    "processing-instruction()",
    "processing-instruction('pi')",
    "svg:path",
    "svg:svg",
    "svg:*",
    "h:div",
    "h:*",
    "math:mi",
    "x:y",
];
const AXES: &[&str] = &[
    "child",
    "descendant",
    "descendant-or-self",
    "self",
    "parent",
    "ancestor",
    "ancestor-or-self",
    "following",
    "following-sibling",
    "preceding",
    "preceding-sibling",
    "attribute",
];
const ATTR_NAMES: &[&str] = &[
    "id",
    "Id",
    "class",
    "x",
    "Y",
    "y",
    "lang",
    "xml:lang",
    "xlink:href",
    "href",
    "viewBox",
    "viewbox",
    "definitionURL",
    "definitionurl",
    "encoding",
    "size",
    "type",
    "*",
    "xlink:*",
];
const LITERALS: &[&str] = &[
    "''", "'a'", "'b'", "'c'", "'en'", "'t'", "'1'", "'red'", "' x '",
];

/// An XPath expression, its shape chosen by `pick(n)` (`0..n`) - a seeded
/// generator in the tests, the fuzzer's bytes in the target.
pub fn expression(pick: &mut dyn FnMut(usize) -> usize) -> String {
    let path = |pick: &mut dyn FnMut(usize) -> usize| location_path(pick, 2);
    match pick(12) {
        0..=4 => path(pick),
        5 => format!("count({})", path(pick)),
        6 => format!("string({})", path(pick)),
        7 => {
            let f = [
                "name",
                "local-name",
                "namespace-uri",
                "normalize-space",
                "boolean",
            ][pick(5)];
            format!("{f}({})", path(pick))
        }
        8 => format!("{} | {}", path(pick), path(pick)),
        9 => format!(
            "id({})",
            ["'a'", "'a b'", "'b a c'", "string(//@class)"][pick(4)]
        ),
        10 => format!("({})[{}]", path(pick), ["1", "last()", "2"][pick(3)]),
        _ => format!("{} = {}", path(pick), LITERALS[pick(LITERALS.len())]),
    }
}

fn location_path(pick: &mut dyn FnMut(usize) -> usize, nesting: usize) -> String {
    let mut s = String::from(["/", "//", ".//", "", "", "../", "./"][pick(7)]);
    for i in 0..[1, 1, 2, 3][pick(4)] {
        if i > 0 {
            s.push_str(["/", "//"][pick(2)]);
        }
        s.push_str(&step(pick, nesting));
    }
    s
}

fn step(pick: &mut dyn FnMut(usize) -> usize, nesting: usize) -> String {
    let mut s = match pick(6) {
        0 => format!("@{}", ATTR_NAMES[pick(ATTR_NAMES.len())]),
        /* An abbreviated step takes no predicate. */
        1 => return ["..", "."][pick(2)].to_string(),
        2 | 3 => {
            let axis = AXES[pick(AXES.len())];
            let test = if axis == "attribute" {
                ATTR_NAMES[pick(ATTR_NAMES.len())]
            } else {
                name(pick)
            };
            format!("{axis}::{test}")
        }
        _ => name(pick).to_string(),
    };
    for _ in 0..pick(4).saturating_sub(2) {
        s.push('[');
        s.push_str(&predicate(pick, nesting));
        s.push(']');
    }
    s
}

/// A node test: two times in three one that matches whatever is there, so a
/// path of several steps still reaches nodes.
fn name(pick: &mut dyn FnMut(usize) -> usize) -> &'static str {
    match pick(3) {
        0 => "*",
        1 => "node()",
        _ => NAMES[pick(NAMES.len())],
    }
}

fn predicate(pick: &mut dyn FnMut(usize) -> usize, nesting: usize) -> String {
    let attr = |pick: &mut dyn FnMut(usize) -> usize| ATTR_NAMES[pick(ATTR_NAMES.len())];
    match pick(14) {
        0 => [
            "1",
            "2",
            "last()",
            "position() < 3",
            "position() = last() - 1",
        ][pick(5)]
        .into(),
        1 => format!("@{}", attr(pick)),
        2 => format!("@{} = {}", attr(pick), LITERALS[pick(LITERALS.len())]),
        3 => format!("lang({})", ["'en'", "'EN'", "'e'", "''"][pick(4)]),
        4 => format!("name() = '{}'", NAMES[pick(NAMES.len())]),
        5 => format!("local-name() = '{}'", NAMES[pick(NAMES.len())]),
        6 => format!(
            "namespace-uri() = '{}'",
            [
                "",
                "http://www.w3.org/1999/xhtml",
                "http://www.w3.org/2000/svg"
            ][pick(3)]
        ),
        7 => format!("contains(., {})", LITERALS[pick(LITERALS.len())]),
        8 => format!("count({}) > {}", NAMES[pick(NAMES.len())], pick(3)),
        9 => format!("string-length(.) > {}", pick(3)),
        10 => format!(". = {}", LITERALS[pick(LITERALS.len())]),
        11 if nesting > 0 => format!("not({})", location_path(pick, nesting - 1)),
        12 if nesting > 0 => location_path(pick, nesting - 1),
        _ => "text()".into(),
    }
}

/* ---------- the documents ---------- */

const HTML_TAGS: &[&str] = &[
    "div", "DiV", "p", "a", "b", "i", "li", "ul", "table", "tr", "td", "select", "option",
    "template", "my-el", "x", "title", "span",
];
const SVG_TAGS: &[&str] = &[
    "path",
    "g",
    "desc",
    "foreignObject",
    "linearGradient",
    "svg",
];
const MATH_TAGS: &[&str] = &["mi", "mo", "annotation-xml"];
const DOC_ATTRS: &[&str] = &[
    "id=a",
    "id=b",
    "ID=c",
    "class=c",
    "class='a b'",
    "x=1",
    "Y=2",
    "lang=en",
    "lang=EN-us",
    "xml:lang=en",
    "xlink:href=red",
    "href=t",
    "viewBox='0 0 1 1'",
    "definitionURL=u",
    "encoding=text/html",
    "size=1",
    "type=TEXT",
    "xmlns:x=urn:x",
];
const DOC_TEXTS: &[&str] = &["a", "b", " x ", "en", "t", "1", "red", "", "\n", "&amp;"];

/// A document whose names, attributes and texts are the ones [`expression`]
/// asks about, nested so the axes have somewhere to go - HTML with SVG and
/// MathML subtrees, templates, comments and PIs. `pick` as for
/// [`expression`].
pub fn document(pick: &mut dyn FnMut(usize) -> usize) -> String {
    let mut s = String::new();
    if pick(2) == 0 {
        s.push_str("<!DOCTYPE html>");
    }
    for _ in 0..1 + pick(4) {
        element(pick, &mut s, HTML_TAGS, 4);
    }
    s
}

fn element(pick: &mut dyn FnMut(usize) -> usize, s: &mut String, tags: &[&str], depth: usize) {
    let (tag, tags) = match pick(12) {
        0 if tags == HTML_TAGS => ("svg", SVG_TAGS),
        1 if tags == HTML_TAGS => ("math", MATH_TAGS),
        _ => (tags[pick(tags.len())], tags),
    };
    let inner = match tag {
        "foreignObject" => HTML_TAGS,
        "annotation-xml" => SVG_TAGS,
        _ => tags,
    };
    s.push('<');
    s.push_str(tag);
    for _ in 0..pick(3) {
        s.push(' ');
        s.push_str(DOC_ATTRS[pick(DOC_ATTRS.len())]);
    }
    s.push('>');
    for _ in 0..pick(4) {
        match pick(8) {
            0..=3 if depth > 0 => element(pick, s, inner, depth - 1),
            4 => s.push_str("<!--c-->"),
            5 => s.push_str("<?pi d?>"),
            _ => s.push_str(DOC_TEXTS[pick(DOC_TEXTS.len())]),
        }
    }
    s.push_str("</");
    s.push_str(tag);
    s.push('>');
}

/* ---------- the comparison ---------- */

/// Every node of the walk - pre-order with template contents, each element
/// followed by its attributes - as both sides' answers are numbered.
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

/// What an answer means, independent of the DOM it came from: a node-set as
/// walk positions. `None` for a limit, which either side may reach first -
/// the two walk different structures and charge them differently.
fn show(v: Result<Val, Error>, position: &dyn Fn(Token) -> Option<usize>) -> Option<String> {
    Some(match v {
        Ok(Val::NodeSet(ns)) => format!(
            "{:?}",
            ns.as_slice()
                .iter()
                .map(|&t| position(t))
                .collect::<Vec<_>>()
        ),
        Ok(Val::String(s)) => format!("{:?}", String::from_utf8_lossy(s.as_slice())),
        Ok(Val::Number(n)) => format!("{n}"),
        Ok(Val::Boolean(x)) => format!("{x}"),
        Err(e) if e.status == ErrorKind::Limit => return None,
        Err(e) => format!("error {:?}", e.status),
    })
}

/// Whether `expr`, evaluated over Lexbor's parse of `input` from the
/// document or one of its elements (the `at`th of them, wrapped), answers alike through
/// both `Dom`s, strict and lax, `evaluate` and `evaluate_first`; the
/// difference when not. An expression that does not parse, or an input
/// Lexbor refuses under `DepthLimit::DEFAULT`, is not compared. `Ok(true)`
/// when some answer compared was not trivially empty (a node-set with
/// nodes, a count past 0, a string with text) - so a caller can tell
/// a comparison that saw something from one that only agreed on nothing.
pub fn check(input: &[u8], expr: &str, at: usize) -> Result<bool, String> {
    let Ok(mut lp) = parse_html(input, false, DepthLimit::DEFAULT) else {
        return Ok(false);
    };
    // SAFETY: `lp` owns the document, outlives every handle made here, and
    // nothing edits it.
    let ld = unsafe { lp.raw_doc().as_doc() };
    let copy = copy_document(ld).map_err(|e| format!("copy: {e:?}"))?;
    let ap = arena::HtmlParsed::new(copy, None);
    let (lw, aw) = (lex_walk(ld), arena_walk(ap.doc()));
    if lw.len() != aw.len() {
        return Err(format!("the walks differ: {} and {}", lw.len(), aw.len()));
    }
    let lpos: HashMap<usize, usize> = lw
        .iter()
        .enumerate()
        .map(|(i, n)| (lex::RawNode::from(*n).as_ptr() as usize, i))
        .collect();
    let apos: HashMap<arena::NodeId, usize> =
        aw.iter().enumerate().map(|(i, n)| (n.id(), i)).collect();
    let by_lex = |t: Token| lpos.get(&(t.as_ptr() as usize)).copied();
    let by_arena =
        |t: Token| arena::RawNode::from_word(t.word()).and_then(|r| apos.get(&r.id()).copied());
    let contexts: Vec<usize> = (0..lw.len())
        .filter(|&i| {
            matches!(
                lw[i].node_type(),
                crate::node_type::NodeType::Document | crate::node_type::NodeType::Element
            )
        })
        .collect();
    let at = contexts[at % contexts.len()];
    let lnode = lex::RawNode::from(lw[at]).as_ptr();
    let anode = arena::RawNode::from(aw[at]).word();
    let Some(text) = crate::text::VerifiedText::new(expr) else {
        return Ok(false);
    };
    let mut saw_nodes = false;

    for lax in [false, true] {
        // SAFETY: as above; `lnode` is a node of that document.
        let mut lc = unsafe { crate::lexbor::xpath::context(&mut *lp, Token::html(lnode)) }
            .map_err(|e| format!("context: {:?}", e.status))?;
        let mut ac = arena::xpath::context(&ap, Token::arena(anode));
        for c in [lc.limits_mut(), ac.limits_mut()] {
            c.max_eval_ops = 200_000;
            c.max_nodeset_size = 4096;
        }
        lc.set_lax(lax);
        ac.set_lax(lax);
        for &(pre, uri) in NAMESPACES {
            lc.register_ns(pre, uri).map_err(|_| "register_ns")?;
            ac.register_ns(pre, uri).map_err(|_| "register_ns")?;
        }
        let mut b = Budget::with_limits(lc.limits());
        let Ok(ast) = parse_owned(text, &mut b) else {
            return Ok(false);
        };
        for first in [false, true] {
            let (l, a) = if first {
                (lc.evaluate_first(&ast, None), ac.evaluate_first(&ast, None))
            } else {
                (lc.evaluate(&ast, None), ac.evaluate(&ast, None))
            };
            if let (Some(l), Some(a)) = (show(l, &by_lex), show(a, &by_arena)) {
                saw_nodes |= !matches!(l.as_str(), "[]" | "0" | "\"\"" | "false" | "NaN")
                    && !l.starts_with("error");
                if l != a {
                    return Err(format!(
                        "{expr} (lax {lax}, first {first}) from node {at}\n--- lexbor\n{l}\n--- arena\n{a}"
                    ));
                }
            }
        }
    }
    Ok(saw_nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexbor::tree_port_tests::{random_input, Rng};

    #[test]
    fn the_generator_makes_expressions_that_parse() {
        let mut rng = Rng(7);
        let mut parsed = 0;
        for _ in 0..1000 {
            let e = expression(&mut |n| rng.below(n));
            let text = crate::text::VerifiedText::new(&e).expect("text");
            let limits = crate::xpath::limits::Limits::default();
            let mut b = Budget::with_limits(limits);
            if parse_owned(text, &mut b).is_ok() {
                parsed += 1;
            }
        }
        assert!(parsed > 950, "only {parsed} of 1000 parse");
    }

    #[test]
    fn xpath_answers_alike_on_random_documents() {
        let count: u64 = std::env::var("XPATH_DIFF_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000);
        let (mut failures, mut compared, mut saw_nodes) = (Vec::new(), 0, 0);
        for seed in 1..=count {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            /* Mostly documents made for the expressions; some of the tree
             * builder's random inputs, for the shapes only a parse makes. */
            let input = if seed % 4 == 0 {
                random_input(&mut rng)
            } else {
                document(&mut |n| rng.below(n))
            };
            let at = rng.below(64);
            for _ in 0..4 {
                let e = expression(&mut |n| rng.below(n));
                compared += 1;
                match check(input.as_bytes(), &e, at) {
                    Ok(true) => saw_nodes += 1,
                    Ok(false) => {}
                    Err(d) => failures.push(format!("seed {seed}: {input:?}\n{d}")),
                }
            }
            if failures.len() >= 5 {
                break;
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
        /* Agreeing on empty answers proves nothing: a generator that drifted
         * into making only those would pass. About a quarter see one. */
        assert!(
            saw_nodes * 5 > compared,
            "only {saw_nodes} of {compared} comparisons saw an answer"
        );
    }
}
