//! Lexbor's reader for the parser conformance harness (`crate::conformance`),
//! and the tree-construction tests run through it.
//!
//! Lexbor is the parser Makiri ships today and the reference the port is
//! compared against, so it is read into the neutral [`Tree`] exactly as any
//! other parser will be. The tests here check that reader and record where
//! Lexbor stands against the pinned WPT data: every test passes or is listed in
//! `spec/conformance/parser_known_divergences.txt`.

#![allow(unsafe_code)]
// Test-only: the harness builds owned trees and sorted lists with std's
// infallible allocation, which `clippy.toml` bans for the engine so that an
// OOM there raises. Aborting a test process on OOM is fine.
#![allow(clippy::disallowed_methods)]

use crate::conformance::dat::{self, DatTest, Scripting};
use crate::conformance::tree::{self, Attr, Element, Node, Quirks, Tree, MATHML_NS, SVG_NS};
use crate::conformance::{known, tree_construction_dir};
use crate::lexbor::adapter::html::{HtmlNode, NsId};
use crate::lexbor::adapter::post_parse::{empty_html_document, parse_html};
use crate::lexbor::adapter::tree_guard::DepthLimit;
use crate::lexbor::fragment::{FragmentContext, FragmentTag, TransientFragment};
use crate::node_type::NodeType;

/// Parse `input` as a document and read it.
pub fn parse_document(input: &[u8]) -> Tree {
    let parsed = parse_html(input, false, DepthLimit::UNLIMITED).expect("Lexbor parses a document");
    // SAFETY: `parsed` owns the document and outlives every handle read here.
    let doc = unsafe { parsed.raw_doc().as_doc() };
    let quirks = match doc.compat_mode() {
        0 => Quirks::NoQuirks,
        1 => Quirks::Quirks,
        _ => Quirks::LimitedQuirks,
    };
    Tree {
        quirks: Some(quirks),
        children: doc.as_node().children().map(read).collect(),
    }
}

/// Parse `input` as a fragment in `context` (`td`, or `svg desc` for a
/// foreign context element) and read the nodes it produced.
pub fn parse_fragment(input: &[u8], context: &str) -> Tree {
    let host = empty_html_document(0).expect("Lexbor makes a document");
    // SAFETY: `host` owns the document and outlives the parse and the reads.
    let doc = unsafe { host.raw_doc().as_doc() };
    let (ns, name) = match context.split_once(' ') {
        Some(("svg", local)) => (NsId::SVG, local),
        Some(("math", local)) => (NsId::MATH, local),
        Some((ns, _)) => panic!("unknown fragment context namespace {ns:?}"),
        None => (NsId::HTML, context),
    };
    /* A name Lexbor's static table lacks (`svg g`) is a context with no
     * special meaning to the tree builder; intern it in the host document. */
    let lower = name.to_ascii_lowercase();
    let tag = doc
        .tag_id(lower.as_bytes())
        .or_else(|| doc.intern_tag(lower.as_bytes()))
        .unwrap_or_else(|| panic!("Lexbor could not intern tag {name:?}"));
    let ctx = FragmentContext::Tag {
        doc: host.raw_doc(),
        at: FragmentTag { tag, ns: Some(ns) },
    };
    // SAFETY: `host` is live for the call and the input is only read.
    let frag = unsafe { TransientFragment::parse(input, false, &ctx, DepthLimit::UNLIMITED) }
        .expect("Lexbor parses a fragment");
    // SAFETY: `frag` owns the parsed nodes and outlives the reads.
    let root = unsafe { frag.root().as_node() };
    Tree {
        quirks: None,
        children: root.children().map(read).collect(),
    }
}

fn bytes(b: &[u8]) -> Vec<u8> {
    b.to_vec()
}

/// `prefix:local` split at the first colon.
fn split_qname(q: &[u8]) -> (Option<Vec<u8>>, Vec<u8>) {
    match q.iter().position(|&c| c == b':') {
        Some(i) => (Some(q[..i].to_vec()), q[i + 1..].to_vec()),
        None => (None, q.to_vec()),
    }
}

fn read(n: HtmlNode<'_>) -> Node {
    match n.node_type() {
        NodeType::Element => {
            let el = n.element().expect("an element node is an element");
            let prefix = if n.has_prefix() {
                split_qname(el.qualified_name()).0
            } else {
                None
            };
            let attrs = el
                .attrs()
                .map(|a| match a.own_ns_uri() {
                    Some(uri) => {
                        let (prefix, _) = split_qname(a.qualified_name());
                        Attr {
                            ns: Some(bytes(uri)),
                            prefix,
                            local: bytes(a.dom_local_name()),
                            value: bytes(a.value()),
                        }
                    }
                    None => Attr {
                        ns: None,
                        prefix: None,
                        local: bytes(a.qualified_name()),
                        value: bytes(a.value()),
                    },
                })
                .collect();
            Node::Element(Element {
                ns: n.ns_uri().map(bytes),
                prefix,
                local: bytes(el.dom_local_name()),
                attrs,
                children: n.children().map(read).collect(),
                contents: n
                    .template_content()
                    .map(|c| c.children().map(read).collect()),
            })
        }
        NodeType::Text | NodeType::CDataSection => Node::Text(bytes(n.data().unwrap_or_default())),
        NodeType::Comment => Node::Comment(bytes(n.data().unwrap_or_default())),
        NodeType::Pi => Node::Pi {
            target: bytes(n.pi_target().unwrap_or_default()),
            data: bytes(n.data().unwrap_or_default()),
        },
        NodeType::DocumentType => Node::Doctype {
            name: bytes(n.node_name()),
            public_id: bytes(n.doctype_public_id().unwrap_or_default()),
            system_id: bytes(n.doctype_system_id().unwrap_or_default()),
        },
        other => panic!("a parser made a {other:?} node"),
    }
}

/// Run one `.dat` test through Lexbor: its html5lib dump, or `None` for a
/// scripting-on test (Makiri parses with scripting disabled).
pub fn run(t: &DatTest) -> Option<String> {
    if t.scripting == Scripting::On {
        return None;
    }
    let tree = match &t.fragment {
        Some(ctx) => parse_fragment(&t.data, ctx),
        None => parse_document(&t.data),
    };
    Some(tree::html5lib(&tree))
}

#[test]
fn reads_namespaces_attributes_and_template_contents() {
    let t = parse_document(b"<!DOCTYPE html><svg viewBox='0 0 1 1'><a xlink:href=x></a></svg><template><b>t</b></template><?pi data?>");
    assert_eq!(t.quirks, Some(Quirks::NoQuirks));
    let dump = tree::full(&t);
    assert!(dump.contains("<svg svg>"), "{dump}");
    assert!(dump.contains("@- viewBox=\"0 0 1 1\""), "{dump}");
    assert!(dump.contains("@xlink xlink:href=\"x\""), "{dump}");
    assert!(dump.contains("#contents\n|         <html b>"), "{dump}");
    assert!(dump.contains("#pi \"pi\" \"data\""), "{dump}");
    let Node::Element(html) = &t.children[1] else {
        panic!("{dump}")
    };
    assert_eq!(html.ns.as_deref(), Some(crate::conformance::tree::HTML_NS));
}

#[test]
fn reads_quirks_mode_and_foreign_fragment_contexts() {
    assert_eq!(parse_document(b"<p>").quirks, Some(Quirks::Quirks));
    assert_eq!(
        parse_document(b"<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\">").quirks,
        Some(Quirks::LimitedQuirks)
    );
    let f = parse_fragment(b"<circle></circle>x", "svg desc");
    /* `desc` is an HTML integration point: a tag in it is HTML again. */
    assert_eq!(tree::html5lib(&f), "| <circle>\n| \"x\"");
    let f = parse_fragment(b"<circle/>", "svg g");
    let Node::Element(e) = &f.children[0] else {
        panic!()
    };
    assert_eq!(e.ns.as_deref(), Some(SVG_NS));
    let f = parse_fragment(b"<mi>", "math math");
    let Node::Element(e) = &f.children[0] else {
        panic!()
    };
    assert_eq!(e.ns.as_deref(), Some(MATHML_NS));
}

/// Every WPT tree-construction test, through Lexbor. Needs the fetched data
/// (`rake conformance:parser` fetches it and runs this).
#[test]
#[ignore = "needs the fetched WPT data; run by `rake conformance:parser`"]
fn wpt_tree_construction() {
    let dir = tree_construction_dir();
    let tests = dat::read_dir(&dir).unwrap_or_else(|| {
        panic!(
            "no .dat files in {} - run `rake conformance:parser`",
            dir.display()
        )
    });
    let known = known::load();
    let (mut pass, mut skipped, mut known_hit) = (0, 0, 0);
    let mut failures = Vec::new();
    let mut now_passing = Vec::new();
    for t in &tests {
        let Some(actual) = run(t) else {
            skipped += 1;
            continue;
        };
        let listed = known.get(&t.key());
        match (actual == t.document, listed) {
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
        "WPT tree construction through Lexbor: {} tests, {pass} pass, {known_hit} known, \
         {skipped} scripting-on skipped, {} fail, {} now passing",
        tests.len(),
        failures.len(),
        now_passing.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(
        now_passing.is_empty(),
        "listed as known but now passing - remove from the list and the table: {now_passing:?}"
    );
    assert_eq!(
        known_hit,
        known.len(),
        "a listed test was not found in the data"
    );
}
