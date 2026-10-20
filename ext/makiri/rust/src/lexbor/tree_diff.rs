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

use crate::compat::{as_lexbor, explains};
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
    let mut out = format!("compat {}\n", d.compat_mode().lexbor());
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

/// Whether the port, run as Lexbor ran (`compat::as_lexbor`),
/// builds Lexbor's tree for `input`, and its fixes change that tree only
/// where `compat::explains` admits it; the difference when not.
pub fn check(input: &[u8], offsets: bool) -> Result<(), String> {
    let l = lexbor_dump(input, offsets);
    let p = as_lexbor(|| port_dump(input, offsets));
    /* The fixes' trees, offsets aside (L1 changes only those, and is
     * checked on its own below). */
    let c = as_lexbor(|| port_dump(input, false));
    if let (Some(c), Some(f)) = (&c, port_dump(input, false)) {
        if *c != f && explains(input, None).is_empty() {
            return Err(format!(
                "a fix changed the tree where no entry explains it\n{}",
                first_difference(c, &f)
            ));
        }
    }
    if offsets {
        offsets_name_their_tags(input)?;
    }
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
    let f = crate::html::tree::parse_fragment(
        input,
        false,
        tag,
        ns,
        options,
        crate::html::tree::ContextFacts::default(),
        limit,
    )
    .map_err(|e| format!("{e:?}"))?;
    Ok(dump_children(&f.doc, f.root, false))
}

pub fn check_fragment(input: &[u8], context: &str) -> Result<(), String> {
    let (tag, ns) = context_ids(context);
    let l = lexbor_fragment(input, tag, ns, DepthLimit::UNLIMITED);
    let p = as_lexbor(|| port_fragment(input, tag, ns, DepthLimit::UNLIMITED));
    if let (Ok(c), Ok(f)) = (&p, port_fragment(input, tag, ns, DepthLimit::UNLIMITED)) {
        if *c != f && explains(input, Some(context)).is_empty() {
            return Err(format!(
                "a fix changed the fragment where no entry explains it\n{}",
                first_difference(c, &f)
            ));
        }
    }
    match (l, p) {
        (Ok(l), Ok(p)) if l == p => Ok(()),
        (Ok(l), Ok(p)) => Err(first_difference(&l, &p)),
        (l, p) => Err(format!("lexbor: {:?}, port: {:?}", l.err(), p.err())),
    }
}

/// The source-offset rule (L1 in docs/parser_differences.ja.md), checked on
/// its own terms: every element the port stamps sits at a start tag of its
/// own name in the parsed text - never a wrong line. (`<image>` makes an
/// `img`.) Which elements are stamped is the rule's; this holds it to the
/// one property `Node#line` promises.
pub fn offsets_name_their_tags(input: &[u8]) -> Result<(), String> {
    let Some(parsed) = crate::html::tree::parse_html(input, false, DepthLimit::UNLIMITED).ok()
    else {
        return Ok(());
    };
    let text = crate::utf8_input::sanitize(input, false).expect("sanitize");
    let text = text.as_slice();
    let d = parsed.arena();
    let mut stack = vec![NodeId::DOCUMENT];
    while let Some(n) = stack.pop() {
        if let Some(c) = d.template_content(n) {
            stack.push(c);
        }
        let mut c = d.first_child(n);
        while let Some(x) = c {
            stack.push(x);
            c = d.next(x);
        }
        if d.kind(n) != NodeType::Element {
            continue;
        }
        let Some(o) = d.source_offset(n) else {
            continue;
        };
        let name = d.names.tag_name(d.local(n) as usize).unwrap_or_default();
        /* A start tag's offset is Lexbor's `token->begin`: the byte after
         * its `<`. */
        let after = text.get(o..).unwrap_or_default();
        /* As the tokenizer reads a tag name: up to whitespace, `/` or `>`,
         * with U+0000 as U+FFFD. */
        let mut written = Vec::new();
        for &b in after
            .iter()
            .take_while(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | b'/' | b'>'))
        {
            if b == 0 {
                written.extend_from_slice("\u{FFFD}".as_bytes());
            } else {
                written.push(b);
            }
        }
        let named = written.eq_ignore_ascii_case(name)
            || (name == b"img" && written.eq_ignore_ascii_case(b"image"));
        /* `</br>` is handled as a `<br>` start tag, which makes the element:
         * its offset is after `</`. */
        let opens = o >= 1 && text.get(o - 1) == Some(&b'<');
        let br_end = name == b"br" && o >= 2 && text.get(o - 2..o) == Some(&b"</"[..]);
        if !(opens || br_end) || !named {
            return Err(format!(
                "<{}> stamped at {o}, where the text reads {:?}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(&text[o.min(text.len())..(o + 24).min(text.len())])
            ));
        }
    }
    Ok(())
}
