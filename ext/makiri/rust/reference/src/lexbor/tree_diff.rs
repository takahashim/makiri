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

use crate::html::tree::context_ids;
use std::fmt::Write as _;

use crate::compat::{as_lexbor, explains};
use crate::html::dom::{Document, NodeId, NodeType};
use crate::lexbor::adapter::arena_copy::{copy_children, copy_document};
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::adapter::tree_guard::ParseLimits;

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
    let parsed = parse_html(input, false, ParseLimits::UNLIMITED).ok()?;
    let doc = copy_document(parsed.doc()).expect("the copy");
    Some(dump(&doc, offsets))
}

/// The port's tree.
pub fn port_dump(input: &[u8], offsets: bool) -> Option<String> {
    let parsed = crate::html::tree::parse_html(input, false, ParseLimits::UNLIMITED).ok()?;
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

pub fn lexbor_fragment(
    input: &[u8],
    tag: usize,
    ns: usize,
    limit: ParseLimits,
) -> Result<String, String> {
    use crate::lexbor::adapter::html as lex;
    let at = crate::lexbor::fragment::FragmentTag {
        tag: lex::TagId::from_raw(tag).unwrap(),
        ns: lex::NsId::from_raw(ns),
    };
    let frag =
        crate::lexbor::fragment::TransientFragment::parse_standalone(input, false, Some(at), limit)
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
    limit: ParseLimits,
) -> Result<String, String> {
    let f = crate::html::tree::parse_fragment(
        input,
        false,
        &crate::html::tree::ContextElement::named(tag, ns),
        limit,
    )
    .map_err(|e| format!("{e:?}"))?;
    Ok(dump_children(&f.doc, f.root, false))
}

pub fn check_fragment(input: &[u8], context: &str) -> Result<(), String> {
    let (tag, ns) = context_ids(context).expect("an html5lib fragment context");
    let l = lexbor_fragment(input, tag, ns, ParseLimits::UNLIMITED);
    let p = as_lexbor(|| port_fragment(input, tag, ns, ParseLimits::UNLIMITED));
    if let (Ok(c), Ok(f)) = (&p, port_fragment(input, tag, ns, ParseLimits::UNLIMITED)) {
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
    let Some(parsed) = crate::html::tree::parse_html(input, false, ParseLimits::UNLIMITED).ok()
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

/* ---------- serialization ---------- */

/// Up to this many nodes of a tree are serialized in every form; a larger
/// tree is sampled evenly, so a deep one does not make the check quadratic.
const SERIALIZED_NODES: usize = 64;

fn serial_text(b: Option<crate::ceiling_buf::Buf>) -> String {
    match b {
        Some(b) => String::from_utf8_lossy(b.as_slice()).into_owned(),
        None => "<none>".into(),
    }
}

/// Every form (`deep`, `pretty`) Lexbor's serializer writes for `n`.
fn forms_by_lexbor(n: crate::lexbor::adapter::html::HtmlNode<'_>) -> [String; 4] {
    [(false, false), (true, false), (false, true), (true, true)]
        .map(|(deep, pretty)| serial_text(crate::lexbor::serialize::serialize(n, deep, pretty)))
}

/// The same four forms from the arena's serializer, run as Lexbor serialized.
fn forms_by_port(n: crate::html::dom::HtmlNode<'_>) -> [String; 4] {
    use crate::html::dom::serialize::{serialize, Layout, Scope};
    [
        (Scope::Outer, Layout::Plain),
        (Scope::Inner, Layout::Plain),
        (Scope::Outer, Layout::Pretty),
        (Scope::Inner, Layout::Pretty),
    ]
    .map(|(scope, layout)| serial_text(as_lexbor(|| serialize(n, scope, layout))))
}

fn compare_forms(what: &str, l: [String; 4], p: [String; 4]) -> Result<(), String> {
    const FORMS: [&str; 4] = ["outer", "inner", "outer pretty", "inner pretty"];
    for ((l, p), form) in l.iter().zip(&p).zip(FORMS) {
        if l != p {
            return Err(format!(
                "{form} serialization of {what} differs\n--- lexbor\n{l:?}\n--- port\n{p:?}"
            ));
        }
    }
    Ok(())
}

/// Whether the port's tree for `input`, parsed and serialized as Lexbor ran,
/// serializes as Lexbor's does - the document and a spread of its nodes, in
/// every form. [`check`] holds the trees equal; this holds the serializer to
/// Lexbor's over the same arbitrary trees.
pub fn check_serialization(input: &[u8]) -> Result<(), String> {
    use crate::html::dom::HtmlDoc as ArenaDoc;
    let Ok(lp) = parse_html(input, false, ParseLimits::UNLIMITED) else {
        return Ok(());
    };
    let Some(pp) =
        as_lexbor(|| crate::html::tree::parse_html(input, false, ParseLimits::UNLIMITED).ok())
    else {
        return Ok(());
    };
    let l: Vec<_> = lp.doc().as_node().subtree_with_contents().collect();
    let a: Vec<_> = ArenaDoc::of(pp.arena())
        .as_node()
        .subtree_with_contents()
        .collect();
    if l.len() != a.len() {
        return Err(format!(
            "the walks differ: {} and {} nodes",
            l.len(),
            a.len()
        ));
    }
    let stride = l.len().div_ceil(SERIALIZED_NODES).max(1);
    for i in (0..l.len()).step_by(stride) {
        compare_forms(
            &format!("node {i} ({:?})", l[i].node_type()),
            forms_by_lexbor(l[i]),
            forms_by_port(a[i]),
        )?;
    }
    Ok(())
}

/// [`check_serialization`] of a fragment: its contents, as `innerHTML` reads
/// them, both plain and pretty.
pub fn check_fragment_serialization(input: &[u8], context: &str) -> Result<(), String> {
    use crate::html::dom::serialize::{serialize, Layout, Scope};
    use crate::html::dom::{HtmlDoc as ArenaDoc, RawNode};
    use crate::lexbor::adapter::html as lex;
    let (tag, ns) = context_ids(context).expect("an html5lib fragment context");
    let at = crate::lexbor::fragment::FragmentTag {
        tag: lex::TagId::from_raw(tag).unwrap(),
        ns: lex::NsId::from_raw(ns),
    };
    let Ok(lf) = crate::lexbor::fragment::TransientFragment::parse_standalone(
        input,
        false,
        Some(at),
        ParseLimits::UNLIMITED,
    ) else {
        return Ok(());
    };
    let Ok(pf) = as_lexbor(|| {
        crate::html::tree::parse_fragment(
            input,
            false,
            &crate::html::tree::ContextElement::named(tag, ns),
            ParseLimits::UNLIMITED,
        )
    }) else {
        return Ok(());
    };
    let root = ArenaDoc::of(&pf.doc).node(RawNode::new(pf.doc.stamp(), pf.root));
    for pretty in [false, true] {
        let l = serial_text(lf.with_root(|r| crate::lexbor::serialize::serialize(r, true, pretty)));
        let layout = if pretty {
            Layout::Pretty
        } else {
            Layout::Plain
        };
        let p = serial_text(as_lexbor(|| serialize(root, Scope::Inner, layout)));
        if l != p {
            return Err(format!(
                "fragment serialization (pretty {pretty}) in {context} differs\n--- lexbor\n{l:?}\n--- port\n{p:?}"
            ));
        }
    }
    Ok(())
}

/* ---------- the guard ---------- */

/// Whether both parsers accept `input` as a document under `limit`, or
/// refuse it the same way. [`check`] runs unlimited, so a refusal on one
/// side only is never a difference there; this is where it is one.
pub fn check_verdict(input: &[u8], limit: ParseLimits) -> Result<(), String> {
    let l = parse_html(input, false, limit).map(|_| ());
    let p = as_lexbor(|| crate::html::tree::parse_html(input, false, limit).map(|_| ()));
    if l == p {
        Ok(())
    } else {
        Err(format!("under {limit:?}: lexbor {l:?}, port {p:?}"))
    }
}

/// [`check_verdict`] for a fragment in `context`; where both accept, the
/// trees must agree too.
pub fn check_fragment_verdict(
    input: &[u8],
    context: &str,
    limit: ParseLimits,
) -> Result<(), String> {
    let (tag, ns) = context_ids(context).expect("an html5lib fragment context");
    let l = lexbor_fragment(input, tag, ns, limit);
    let p = as_lexbor(|| port_fragment(input, tag, ns, limit));
    match (l, p) {
        (Ok(l), Ok(p)) if l == p => Ok(()),
        (Ok(l), Ok(p)) => Err(format!("under {limit:?}\n{}", first_difference(&l, &p))),
        (Err(_), Err(_)) => Ok(()),
        (l, p) => Err(format!(
            "under {limit:?}: lexbor {:?}, port {:?}",
            l.err(),
            p.err()
        )),
    }
}
