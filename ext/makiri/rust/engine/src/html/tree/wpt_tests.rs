//! The WPT tree-construction data through the parser port, held to the
//! pinned expectations: the port's own conformance, which needs no Lexbor
//! (`rake conformance:parser` runs it, with the fetched data).
#![allow(clippy::disallowed_methods)] /* test inputs */

use crate::conformance::dat::{self, Scripting};
use crate::conformance::tree_construction_dir;
use crate::html::tree::context_ids;
use crate::html::tree::guard::ParseLimits;

/// One WPT test through the port, `None` for a scripting-on test.
fn port_run(t: &dat::DatTest) -> Option<String> {
    use crate::conformance::tree::{self, Quirks, Tree};
    use crate::html::dom::conformance::{read_children, read_document};
    if t.scripting == Scripting::On {
        return None;
    }
    let tree = match &t.fragment {
        Some(ctx) => {
            let (tag, ns) = context_ids(ctx).expect("an html5lib fragment context");
            let f = crate::html::tree::parse_fragment(
                &t.data,
                false,
                &crate::html::tree::ContextElement::named(tag, ns),
                ParseLimits::UNLIMITED,
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
