//! Inclusive Canonical XML 1.0 output.
//!
//! Its namespace handling is not the XML writer's: c14n RENDERS the declarations
//! the document holds (§2.2's rendered-prefix rules) rather than planning a
//! prefix per name, so the two share the output buffer and the escape table and
//! nothing else.

#![forbid(unsafe_code)]

use super::out::{field, put, C14N, W};
use crate::cbuf::Buf;
use crate::falloc::Reserve;
use crate::xml::model::{Document as XmlDoc, NodeId, NodeType, MAX_DEPTH};
use crate::xml::qname::xmlns_prefix;

fn xmlns_decl(doc: &XmlDoc, a: NodeId) -> Option<(&[u8], &[u8])> {
    let p = xmlns_prefix(doc.qname(a))?;
    Some((p, field(doc, doc.node(a).value)))
}

struct Ns<'d> {
    prefix: &'d [u8],
    uri: &'d [u8],
}

fn nearest<'d>(doc: &'d XmlDoc, node: NodeId, prefix: &[u8]) -> Option<&'d [u8]> {
    let mut e = Some(node);
    while let Some(id) = e {
        if doc.type_(id) == Some(NodeType::Element) {
            let mut a = doc.attrs(id);
            while let Some(at) = a {
                if let Some((p, u)) = xmlns_decl(doc, at) {
                    if p == prefix {
                        return Some(u);
                    }
                }
                a = doc.next(at);
            }
        }
        e = doc.parent(id);
    }
    None
}

fn namespaces(doc: &XmlDoc, n: NodeId, is_apex: bool) -> Result<Vec<Ns<'_>>, ()> {
    let mut out: Vec<Ns> = Vec::new();
    let mut default_seen = false;
    let mut e = Some(n);
    while let Some(id) = e {
        if doc.type_(id) == Some(NodeType::Element) {
            let mut a = doc.attrs(id);
            while let Some(at) = a {
                if let Some((p, u)) = xmlns_decl(doc, at) {
                    if p != b"xml" {
                        let keep = if is_apex {
                            if p.is_empty() {
                                let first = !default_seen;
                                default_seen = true;
                                first && !u.is_empty()
                            } else {
                                !out.iter().any(|x| x.prefix == p)
                            }
                        } else {
                            // Declared above this element, not on it: its own
                            // declaration would always match and never render.
                            let above = doc.parent(id).and_then(|up| nearest(doc, up, p));
                            if above == Some(u) {
                                false
                            } else {
                                !(p.is_empty() && u.is_empty())
                                    || above.is_some_and(|a| !a.is_empty())
                            }
                        };
                        if keep {
                            out.mkr_reserve(1)?;
                            out.push(Ns { prefix: p, uri: u });
                        }
                    }
                }
                a = doc.next(at);
            }
        }
        if !is_apex {
            break;
        }
        e = doc.parent(id);
    }
    /* In place (see clippy.toml): a prefix is declared once per element, so
     * the keys are distinct and stability would buy nothing. */
    out.sort_unstable_by(|a, b| a.prefix.cmp(b.prefix));
    Ok(out)
}

pub(super) fn node(b: &mut Buf, doc: &XmlDoc, n: NodeId, is_apex: bool, comments: bool, depth: u32) -> W {
    match doc.type_(n) {
        Some(NodeType::Element) => {
            if depth as usize >= MAX_DEPTH {
                return Err(());
            }
            put(b, b"<")?;
            put(b, field(doc, doc.node(n).qname))?;

            for ns in namespaces(doc, n, is_apex)? {
                if ns.prefix.is_empty() {
                    put(b, b" xmlns=\"")?;
                } else {
                    put(b, b" xmlns:")?;
                    put(b, ns.prefix)?;
                    put(b, b"=\"")?;
                }
                C14N.write(b, ns.uri, true)?;
                put(b, b"\"")?;
            }

            let mut attrs: Vec<NodeId> = Vec::new();
            let mut a = doc.attrs(n);
            while let Some(at) = a {
                if xmlns_decl(doc, at).is_none() {
                    attrs.mkr_reserve(1)?;
                    attrs.push(at);
                }
                a = doc.next(at);
            }
            /* In place (see clippy.toml): an element's attributes are distinct
             * by (namespace URI, local name), so stability would buy nothing. */
            attrs.sort_unstable_by(|&x, &y| {
                field(doc, doc.node(x).ns_uri)
                    .cmp(field(doc, doc.node(y).ns_uri))
                    .then_with(|| field(doc, doc.node(x).local).cmp(field(doc, doc.node(y).local)))
            });
            for at in attrs {
                put(b, b" ")?;
                put(b, field(doc, doc.node(at).qname))?;
                put(b, b"=\"")?;
                C14N.write(b, field(doc, doc.node(at).value), true)?;
                put(b, b"\"")?;
            }

            put(b, b">")?;
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                node(b, doc, cid, false, comments, depth + 1)?;
                c = doc.next(cid);
            }
            put(b, b"</")?;
            put(b, field(doc, doc.node(n).qname))?;
            put(b, b">")
        }
        Some(NodeType::Text | NodeType::CData) => {
            C14N.write(b, field(doc, doc.node(n).value), false)
        }
        Some(NodeType::Comment) => {
            if comments {
                put(b, b"<!--")?;
                put(b, field(doc, doc.node(n).value))?;
                put(b, b"-->")?;
            }
            Ok(())
        }
        Some(NodeType::Pi) => {
            put(b, b"<?")?;
            put(b, field(doc, doc.node(n).local))?;
            if doc.node(n).value.len != 0 {
                put(b, b" ")?;
                put(b, field(doc, doc.node(n).value))?;
            }
            put(b, b"?>")
        }
        Some(NodeType::Fragment) => {
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                node(b, doc, cid, false, comments, depth)?;
                c = doc.next(cid);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
