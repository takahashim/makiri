//! A parser-neutral DOM tree and its renderings.
//!
//! A parser's output is read into a [`Tree`] once, and every comparison is
//! between two `Tree`s with `==`: it sees node kinds, every name's namespace,
//! prefix and local name, attributes in DOM order, character data as the
//! nodes divide it, doctype ids, template contents, and the document's quirks
//! mode. The renderings exist for people: [`html5lib`] is the format the test
//! data is written in, and [`full`] shows everything `==` compares, for a
//! readable diff when two trees differ.
//!
//! Trees here are small (test documents) and the walks recurse.

use std::fmt::Write as _;

pub const HTML_NS: &[u8] = b"http://www.w3.org/1999/xhtml";
pub const SVG_NS: &[u8] = b"http://www.w3.org/2000/svg";
pub const MATHML_NS: &[u8] = b"http://www.w3.org/1998/Math/MathML";
pub const XLINK_NS: &[u8] = b"http://www.w3.org/1999/xlink";
pub const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";
pub const XMLNS_NS: &[u8] = b"http://www.w3.org/2000/xmlns/";

/// The document's mode, which the doctype sets and CSS class/id matching reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quirks {
    NoQuirks,
    Quirks,
    LimitedQuirks,
}

/// A parsed document, or the children a fragment parse produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    /// `None` for a fragment, whose mode is its context document's.
    pub quirks: Option<Quirks>,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Doctype {
        name: Vec<u8>,
        /// Empty when missing: the DOM does not tell the two apart.
        public_id: Vec<u8>,
        system_id: Vec<u8>,
    },
    Element(Element),
    Text(Vec<u8>),
    Comment(Vec<u8>),
    Pi {
        target: Vec<u8>,
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// The namespace URI; `None` for no namespace.
    pub ns: Option<Vec<u8>>,
    pub prefix: Option<Vec<u8>>,
    /// The DOM `localName`, case preserved (`foreignObject`).
    pub local: Vec<u8>,
    /// In DOM order.
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
    /// A `<template>`'s contents; `None` for every other element.
    pub contents: Option<Vec<Node>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    /// The attribute's own namespace URI; `None` for no namespace.
    pub ns: Option<Vec<u8>>,
    pub prefix: Option<Vec<u8>>,
    pub local: Vec<u8>,
    pub value: Vec<u8>,
}

fn s(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

fn line(out: &mut String, depth: usize, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("| ");
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(text);
}

/// The html5lib tree-construction dump (the `#document` section of a `.dat`
/// test): one node per line, `| ` and two spaces per level, attributes sorted
/// by their displayed name, foreign elements and the namespaced attributes of
/// foreign elements shown as `prefix local`. No trailing newline.
pub fn html5lib(tree: &Tree) -> String {
    let mut out = String::new();
    for n in &tree.children {
        html5lib_node(&mut out, n, 0);
    }
    out
}

fn html5lib_node(out: &mut String, n: &Node, depth: usize) {
    match n {
        Node::Doctype {
            name,
            public_id,
            system_id,
        } => {
            let t = if public_id.is_empty() && system_id.is_empty() {
                format!("<!DOCTYPE {}>", s(name))
            } else {
                format!(
                    "<!DOCTYPE {} \"{}\" \"{}\">",
                    s(name),
                    s(public_id),
                    s(system_id)
                )
            };
            line(out, depth, &t);
        }
        Node::Text(t) => line(out, depth, &format!("\"{}\"", s(t))),
        Node::Comment(c) => line(out, depth, &format!("<!-- {} -->", s(c))),
        Node::Pi { target, data } => line(out, depth, &format!("<?{} {}?>", s(target), s(data))),
        Node::Element(e) => {
            let ns = e.ns.as_deref();
            let open = if ns == Some(SVG_NS) {
                format!("<svg {}>", s(&e.local))
            } else if ns == Some(MATHML_NS) {
                format!("<math {}>", s(&e.local))
            } else {
                format!("<{}>", s(&e.local))
            };
            line(out, depth, &open);
            let mut attrs: Vec<(String, &Attr)> =
                e.attrs.iter().map(|a| (html5lib_attr_name(a), a)).collect();
            attrs.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, a) in attrs {
                line(out, depth + 1, &format!("{}=\"{}\"", name, s(&a.value)));
            }
            match &e.contents {
                Some(contents) => {
                    line(out, depth + 1, "content");
                    for c in contents {
                        html5lib_node(out, c, depth + 2);
                    }
                }
                None => {
                    for c in &e.children {
                        html5lib_node(out, c, depth + 1);
                    }
                }
            }
        }
    }
}

/// An attribute in the parser's "adjust foreign attributes" namespaces is
/// shown as `prefix local`; every other one by its qualified name.
fn html5lib_attr_name(a: &Attr) -> String {
    let ns_prefix = match a.ns.as_deref() {
        Some(XLINK_NS) => Some("xlink"),
        Some(XML_NS) => Some("xml"),
        Some(XMLNS_NS) => Some("xmlns"),
        _ => None,
    };
    match (ns_prefix, &a.prefix) {
        (Some(p), _) => format!("{} {}", p, s(&a.local)),
        (None, Some(p)) => format!("{}:{}", s(p), s(&a.local)),
        (None, None) => s(&a.local).into_owned(),
    }
}

/// Everything `==` compares, one node per line: the quirks mode, each
/// element's namespace URI and prefix, attributes in DOM order with their own
/// namespace, character data with its node boundaries, doctype ids even when
/// empty. For diffs; not a format any test data is written in.
pub fn full(tree: &Tree) -> String {
    let mut out = String::new();
    match tree.quirks {
        Some(q) => {
            let _ = write!(out, "#document {q:?}");
        }
        None => out.push_str("#fragment"),
    }
    for n in &tree.children {
        full_node(&mut out, n, 0);
    }
    out
}

fn ns_label(ns: Option<&[u8]>) -> String {
    match ns {
        None => "-".to_string(),
        Some(HTML_NS) => "html".to_string(),
        Some(SVG_NS) => "svg".to_string(),
        Some(MATHML_NS) => "math".to_string(),
        Some(XLINK_NS) => "xlink".to_string(),
        Some(XML_NS) => "xml".to_string(),
        Some(XMLNS_NS) => "xmlns".to_string(),
        Some(other) => format!("{{{}}}", s(other)),
    }
}

fn qname(prefix: &Option<Vec<u8>>, local: &[u8]) -> String {
    match prefix {
        Some(p) => format!("{}:{}", s(p), s(local)),
        None => s(local).into_owned(),
    }
}

fn full_node(out: &mut String, n: &Node, depth: usize) {
    match n {
        Node::Doctype {
            name,
            public_id,
            system_id,
        } => line(
            out,
            depth,
            &format!(
                "<!DOCTYPE {:?} {:?} {:?}>",
                s(name),
                s(public_id),
                s(system_id)
            ),
        ),
        Node::Text(t) => line(out, depth, &format!("#text {:?}", s(t))),
        Node::Comment(c) => line(out, depth, &format!("#comment {:?}", s(c))),
        Node::Pi { target, data } => {
            line(out, depth, &format!("#pi {:?} {:?}", s(target), s(data)))
        }
        Node::Element(e) => {
            line(
                out,
                depth,
                &format!(
                    "<{} {}>",
                    ns_label(e.ns.as_deref()),
                    qname(&e.prefix, &e.local)
                ),
            );
            for a in &e.attrs {
                line(
                    out,
                    depth + 1,
                    &format!(
                        "@{} {}={:?}",
                        ns_label(a.ns.as_deref()),
                        qname(&a.prefix, &a.local),
                        s(&a.value)
                    ),
                );
            }
            for c in &e.children {
                full_node(out, c, depth + 1);
            }
            if let Some(contents) = &e.contents {
                line(out, depth + 1, "#contents");
                for c in contents {
                    full_node(out, c, depth + 2);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(ns: &[u8], local: &str, attrs: Vec<Attr>, children: Vec<Node>) -> Node {
        Node::Element(Element {
            ns: Some(ns.to_vec()),
            prefix: None,
            local: local.as_bytes().to_vec(),
            attrs,
            children,
            contents: None,
        })
    }

    #[test]
    fn html5lib_dump_sorts_attributes_and_names_foreign_ones() {
        let xlink = Attr {
            ns: Some(XLINK_NS.to_vec()),
            prefix: Some(b"xlink".to_vec()),
            local: b"href".to_vec(),
            value: b"#a".to_vec(),
        };
        let plain = |n: &str| Attr {
            ns: None,
            prefix: None,
            local: n.as_bytes().to_vec(),
            value: b"v".to_vec(),
        };
        let tree = Tree {
            quirks: Some(Quirks::NoQuirks),
            children: vec![
                Node::Doctype {
                    name: b"html".to_vec(),
                    public_id: vec![],
                    system_id: vec![],
                },
                el(
                    HTML_NS,
                    "html",
                    vec![plain("z"), plain("a")],
                    vec![el(
                        SVG_NS,
                        "foreignObject",
                        vec![xlink],
                        vec![Node::Text(b"t\nu".to_vec())],
                    )],
                ),
            ],
        };
        assert_eq!(
            html5lib(&tree),
            "| <!DOCTYPE html>\n| <html>\n|   a=\"v\"\n|   z=\"v\"\n|   <svg foreignObject>\n\
             |     xlink href=\"#a\"\n|     \"t\nu\""
        );
    }

    #[test]
    fn trees_that_dump_alike_can_still_differ() {
        let split = Tree {
            quirks: Some(Quirks::NoQuirks),
            children: vec![Node::Text(b"a".to_vec()), Node::Text(b"b".to_vec())],
        };
        let joined = Tree {
            quirks: Some(Quirks::Quirks),
            children: vec![Node::Text(b"ab".to_vec())],
        };
        assert_ne!(split, joined);
        assert_ne!(full(&split), full(&joined));
    }
}
