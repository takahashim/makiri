//! The ported tree builder (`crate::html::tree`) against Lexbor's, as data:
//! the same input parsed by Lexbor and copied into the arena, and parsed by
//! the port straight into the arena, dumped node by node - every node's
//! kind, names as written, namespaces and prefixes, attributes in order with
//! the `id`/`class` shortcuts, character data, template contents, source
//! offsets, and the document's quirks mode (`docs/rewrite_parser.ja.md`,
//! stage 4). `lexbor::tree_port_tests` and the `html_diff` fuzz target
//! compare the dumps; this is built for them alone (`lexbor-reference`).

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::fmt::Write as _;

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
pub fn first_difference(a: &str, b: &str) -> String {
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

/* ---------- fragments ---------- */

/// A context for both parsers: `td`, or `svg desc` for a foreign one.
pub fn context_ids(context: &str) -> (usize, usize) {
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

pub fn lexbor_fragment(
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

pub fn port_fragment(
    input: &[u8],
    tag: usize,
    ns: usize,
    limit: DepthLimit,
) -> Result<String, String> {
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

pub fn check_fragment(input: &[u8], context: &str) -> Result<(), String> {
    let (tag, ns) = context_ids(context);
    let l = lexbor_fragment(input, tag, ns, DepthLimit::UNLIMITED);
    let p = port_fragment(input, tag, ns, DepthLimit::UNLIMITED);
    match (l, p) {
        (Ok(l), Ok(p)) if l == p => Ok(()),
        (Ok(l), Ok(p)) => Err(first_difference(&l, &p)),
        (l, p) => Err(format!("lexbor: {:?}, port: {:?}", l.err(), p.err())),
    }
}
