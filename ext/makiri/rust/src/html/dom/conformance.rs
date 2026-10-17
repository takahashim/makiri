//! The arena read into the conformance harness's parser-neutral tree
//! (`crate::conformance::tree`), by the same rules `lexbor::conformance_tests`
//! reads Lexbor's: an element's DOM local name is the tail of its qualified
//! name (written, or else the lower-cased local name) at the local name's
//! length, so its case is kept; an attribute in no namespace is named by its
//! whole qualified name.

#![allow(clippy::disallowed_methods)]

use super::{Document, NodeId, NodeType};
use crate::conformance::tree::{Attr, Element, Node, Quirks, Tree};

fn tail<'a>(qualified: &'a [u8], local: &'a [u8]) -> &'a [u8] {
    if qualified.len() >= local.len() {
        &qualified[qualified.len() - local.len()..]
    } else {
        local
    }
}

fn prefix_of(q: &[u8]) -> Option<Vec<u8>> {
    q.iter().position(|&c| c == b':').map(|i| q[..i].to_vec())
}

/// The whole document.
pub fn read_document(d: &Document) -> Tree {
    let quirks = match d.compat_mode {
        0 => Quirks::NoQuirks,
        1 => Quirks::Quirks,
        _ => Quirks::LimitedQuirks,
    };
    Tree {
        quirks: Some(quirks),
        children: children(d, NodeId::DOCUMENT),
    }
}

/// The children of `n`, for a fragment's root.
pub fn read_children(d: &Document, n: NodeId) -> Vec<Node> {
    children(d, n)
}

fn children(d: &Document, n: NodeId) -> Vec<Node> {
    let mut out = Vec::new();
    let mut c = d.first_child(n);
    while let Some(x) = c {
        out.push(read(d, x));
        c = d.next(x);
    }
    out
}

fn read(d: &Document, n: NodeId) -> Node {
    match d.kind(n) {
        NodeType::Element => {
            let local = d.names.tag_name(d.local(n) as usize).unwrap_or_default();
            let qualified = d.element_written(n).unwrap_or(local);
            let prefix = if d.prefix(n) != 0 {
                prefix_of(qualified)
            } else {
                None
            };
            let mut attrs = Vec::new();
            let mut a = d.first_attr(n);
            while let Some(x) = a {
                let alocal = d.names.attr_name(d.local(x) as usize).unwrap_or_default();
                let aq = d.attr_written(x).unwrap_or(alocal);
                let value = d.attr_value(x).unwrap_or_default().to_vec();
                attrs.push(match d.ns_uri(d.attr_own_ns(x)) {
                    Some(uri) => Attr {
                        ns: Some(uri.to_vec()),
                        prefix: if d.prefix(x) != 0 {
                            prefix_of(aq)
                        } else {
                            None
                        },
                        local: tail(aq, alocal).to_vec(),
                        value,
                    },
                    None => Attr {
                        ns: None,
                        prefix: None,
                        local: aq.to_vec(),
                        value,
                    },
                });
                a = d.next_attr(x);
            }
            Node::Element(Element {
                ns: d.ns_uri(d.ns(n)).map(<[u8]>::to_vec),
                prefix,
                local: tail(qualified, local).to_vec(),
                attrs,
                children: children(d, n),
                contents: d.template_content(n).map(|c| children(d, c)),
            })
        }
        NodeType::Text | NodeType::CDataSection => {
            Node::Text(d.data(n).unwrap_or_default().to_vec())
        }
        NodeType::Comment => Node::Comment(d.data(n).unwrap_or_default().to_vec()),
        NodeType::Pi => Node::Pi {
            target: d.pi_target(n).unwrap_or_default().to_vec(),
            data: d.data(n).unwrap_or_default().to_vec(),
        },
        NodeType::DocumentType => {
            let (name, public_id, system_id) = d.doctype(n).unwrap_or_default();
            Node::Doctype {
                name: name.to_vec(),
                public_id: public_id.to_vec(),
                system_id: system_id.to_vec(),
            }
        }
        other => panic!("an arena holds a {other:?} in a tree"),
    }
}
