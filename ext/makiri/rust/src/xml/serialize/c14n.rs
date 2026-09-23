//! Inclusive Canonical XML 1.0 output.
//!
//! Its namespace handling is not the XML writer's: c14n RENDERS the declarations
//! the document holds (§2.2's rendered-prefix rules) rather than planning a
//! prefix per name, so the two share the output buffer and the escape table and
//! nothing else.

#![forbid(unsafe_code)]

use super::bindings::{Bindings, Prefix};
use super::out::{put, put_pi, C14N, W};
use super::Failure;
use crate::cbuf::Buf;
use crate::falloc::Reserve;
use crate::xml::model::{Document as XmlDoc, NodeId, NodeType, FLAG_NS_RESOLVED, MAX_DEPTH};
use crate::xml::qname::xmlns_prefix;
use crate::xml::XML_NS_URI;

fn xmlns_decl(doc: &XmlDoc, a: NodeId) -> Option<(&[u8], &[u8])> {
    let p = xmlns_prefix(doc.qname(a))?;
    Some((p, doc.span(doc.node(a).value)))
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
                            out.falloc_reserve(1)?;
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

/// The Canonical XML writer: the output buffer, the document it reads, and
/// whether comments are kept, so only what varies per node travels as an
/// argument - the same shape `super::xml`'s writer has.
struct Writer<'d, 'b> {
    b: &'b mut Buf,
    doc: &'d XmlDoc,
    comments: bool,
    /// The declarations in scope - the document's own, which is all c14n
    /// renders - to check each name against.
    binds: Bindings<'d>,
    /// Set when a name's namespace is not what those declarations say.
    mismatch: bool,
}

impl<'d> Writer<'d, '_> {
    fn put(&mut self, bytes: &[u8]) -> W {
        put(self.b, bytes)
    }
    fn escape(&mut self, s: &[u8], attr: bool) -> W {
        C14N.write(self.b, s, attr)
    }
    fn qname(&mut self, n: NodeId) -> W {
        let doc = self.doc;
        self.put(doc.span(doc.node(n).qname))
    }

    /// `is_apex` marks the node the canonicalization STARTS at, which renders
    /// every declaration in scope rather than only its own (§2.2).
    fn node(&mut self, n: NodeId, is_apex: bool, depth: u32) -> W {
        let doc = self.doc;
        match doc.type_(n) {
            Some(NodeType::Element) => self.element(n, is_apex, depth),
            Some(NodeType::Text | NodeType::CData) => {
                self.escape(doc.span(doc.node(n).value), false)
            }
            Some(NodeType::Comment) => {
                if self.comments {
                    self.put(b"<!--")?;
                    self.put(doc.span(doc.node(n).value))?;
                    self.put(b"-->")?;
                }
                Ok(())
            }
            Some(NodeType::Pi) => put_pi(self.b, doc, n),
            Some(NodeType::Fragment) => self.children(n, depth),
            _ => Ok(()),
        }
    }

    fn children(&mut self, n: NodeId, depth: u32) -> W {
        let doc = self.doc;
        let mut c = doc.first_child(n);
        while let Some(cid) = c {
            self.node(cid, false, depth)?;
            c = doc.next(cid);
        }
        Ok(())
    }

    /// Push `el`'s own xmlns declarations onto the scope.
    fn push_decls(&mut self, el: NodeId) -> W {
        let doc = self.doc;
        let mut a = doc.attrs(el);
        while let Some(at) = a {
            if let Some((p, u)) = xmlns_decl(doc, at) {
                self.binds.push(Prefix::Own(p), u)?;
            }
            a = doc.next(at);
        }
        Ok(())
    }

    /// What `prefix` means here by the document's declarations: `xml` its own
    /// URI, an undeclared default no namespace. None once the step budget is
    /// spent.
    fn scope_uri(&mut self, prefix: &[u8]) -> Option<&'d [u8]> {
        if prefix == b"xml" {
            return Some(XML_NS_URI);
        }
        match self.binds.lookup(prefix) {
            Some(uri) => Some(uri),
            None if self.binds.exhausted => None,
            None => Some(b""),
        }
    }

    /// Whether the declarations in scope give `n` and its attributes the
    /// namespaces they have. Only for a decided element: an unresolved one
    /// (a detached copy) takes its namespace FROM those declarations.
    fn names_agree(&mut self, n: NodeId) -> Result<bool, ()> {
        let doc = self.doc;
        if doc.node(n).flags & FLAG_NS_RESOLVED == 0 {
            return Ok(true);
        }
        let el_prefix = doc.span(doc.node(n).prefix);
        if self.scope_uri(el_prefix).ok_or(())? != doc.span(doc.node(n).ns_uri) {
            return Ok(false);
        }
        let mut a = doc.attrs(n);
        while let Some(at) = a {
            if xmlns_decl(doc, at).is_none() {
                let (prefix, uri) = (doc.span(doc.node(at).prefix), doc.span(doc.node(at).ns_uri));
                let expected = if prefix.is_empty() {
                    &b""[..]
                } else {
                    self.scope_uri(prefix).ok_or(())?
                };
                if expected != uri {
                    return Ok(false);
                }
            }
            a = doc.next(at);
        }
        Ok(true)
    }

    /// The scope for the apex: every ancestor's declarations, outermost first,
    /// since a subtree renders (and is read) under all of them.
    fn push_ancestor_decls(&mut self, n: NodeId) -> W {
        let doc = self.doc;
        let mut chain: Vec<NodeId> = Vec::new();
        let mut up = doc.parent(n);
        while let Some(id) = up {
            if doc.type_(id) == Some(NodeType::Element) {
                chain.falloc_reserve(1)?;
                chain.push(id);
            }
            up = doc.parent(id);
        }
        for &id in chain.iter().rev() {
            self.push_decls(id)?;
        }
        Ok(())
    }

    fn element(&mut self, n: NodeId, is_apex: bool, depth: u32) -> W {
        if depth as usize >= MAX_DEPTH {
            return Err(());
        }
        let base = self.binds.len();
        if is_apex {
            self.push_ancestor_decls(n)?;
        }
        let r = self.element_in_scope(n, is_apex, depth);
        self.binds.truncate(base);
        r
    }

    /// [`element`](Self::element) with `n`'s own declarations in scope; the
    /// caller owns the scope, so an early `?` cannot unbalance it.
    fn element_in_scope(&mut self, n: NodeId, is_apex: bool, depth: u32) -> W {
        let doc = self.doc;
        self.push_decls(n)?;
        if !self.names_agree(n)? {
            self.mismatch = true;
            return Err(());
        }
        self.put(b"<")?;
        self.qname(n)?;

        for ns in namespaces(doc, n, is_apex)? {
            if ns.prefix.is_empty() {
                self.put(b" xmlns=\"")?;
            } else {
                self.put(b" xmlns:")?;
                self.put(ns.prefix)?;
                self.put(b"=\"")?;
            }
            self.escape(ns.uri, true)?;
            self.put(b"\"")?;
        }

        for at in sorted_attributes(doc, n)? {
            self.put(b" ")?;
            self.qname(at)?;
            self.put(b"=\"")?;
            self.escape(doc.span(doc.node(at).value), true)?;
            self.put(b"\"")?;
        }

        self.put(b">")?;
        let mut c = doc.first_child(n);
        while let Some(cid) = c {
            self.node(cid, false, depth + 1)?;
            c = doc.next(cid);
        }
        self.put(b"</")?;
        self.qname(n)?;
        self.put(b">")
    }
}

/// §3.3: an element's non-declaration attributes, ordered by namespace URI then
/// local name.
fn sorted_attributes(doc: &XmlDoc, n: NodeId) -> Result<Vec<NodeId>, ()> {
    let mut attrs: Vec<NodeId> = Vec::new();
    let mut a = doc.attrs(n);
    while let Some(at) = a {
        if xmlns_decl(doc, at).is_none() {
            attrs.falloc_reserve(1)?;
            attrs.push(at);
        }
        a = doc.next(at);
    }
    /* In place (see clippy.toml): an element's attributes are distinct by
     * (namespace URI, local name), so stability would buy nothing. */
    attrs.sort_unstable_by(|&x, &y| {
        doc.span(doc.node(x).ns_uri)
            .cmp(doc.span(doc.node(y).ns_uri))
            .then_with(|| doc.span(doc.node(x).local).cmp(doc.span(doc.node(y).local)))
    });
    Ok(attrs)
}

/// Write `n` as Inclusive Canonical XML 1.0 into `b`.
///
/// The whole of this module's surface. For the Document node that is the root
/// element plus the top-level PIs (and comments, when asked for) on their own
/// lines before and after it.
pub(super) fn write(b: &mut Buf, doc: &XmlDoc, n: NodeId, comments: bool) -> Result<(), Failure> {
    let mut w = Writer {
        b,
        doc,
        comments,
        binds: Bindings::new(),
        mismatch: false,
    };
    let r = (|| -> W {
        if doc.type_(n) != Some(NodeType::Document) {
            return w.node(n, true, 0);
        }
        let mut seen_root = false;
        let mut c = doc.first_child(n);
        while let Some(cid) = c {
            let ty = doc.type_(cid);
            if ty == Some(NodeType::Element) {
                w.node(cid, true, 0)?;
                seen_root = true;
            } else if ty == Some(NodeType::Pi) || (ty == Some(NodeType::Comment) && comments) {
                if seen_root {
                    w.put(b"\n")?;
                }
                w.node(cid, false, 0)?;
                if !seen_root {
                    w.put(b"\n")?;
                }
            }
            c = doc.next(cid);
        }
        Ok(())
    })();
    r.map_err(|()| {
        if w.mismatch {
            Failure::NamespaceMismatch
        } else if w.binds.exhausted {
            Failure::NamespaceBudget
        } else {
            Failure::Output
        }
    })
}
