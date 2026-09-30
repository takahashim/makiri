//! Inclusive Canonical XML 1.0 output.
//!
//! Its namespace handling is not the XML writer's: c14n RENDERS the declarations
//! the document holds (§2.2's rendered-prefix rules) rather than planning a
//! prefix per name, so the two share the output buffer and the escape table and
//! nothing else. Where a name's namespace was set rather than declared, it adds
//! the declaration of that name's own prefix (`Writer::fixups`), and never
//! invents one.

#![forbid(unsafe_code)]

use super::bindings::{Bindings, Prefix};
use super::out::{put, put_comment, put_pi, C14N, W};
use super::Failure;
use crate::cbuf::Buf;
use crate::falloc::{OomResult, VecPush};
use crate::xml::model::{ArenaKind, Document as XmlDoc, NodeFlags, NodeId, MAX_DEPTH};

fn xmlns_decl(doc: &XmlDoc, a: NodeId) -> Option<(&[u8], &[u8])> {
    let p = doc.decl_prefix(a)?;
    Some((p, doc.span(doc.node(a).value)))
}

struct Ns<'d> {
    prefix: &'d [u8],
    uri: &'d [u8],
}

/// In place (see clippy.toml): a prefix is rendered once per element, so the
/// keys are distinct and stability would buy nothing.
fn sort_by_prefix(out: &mut [Ns<'_>]) {
    out.sort_unstable_by(|a, b| a.prefix.cmp(b.prefix));
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
            Some(ArenaKind::Element) => self.element(n, is_apex, depth),
            Some(ArenaKind::Text | ArenaKind::CDataSection) => {
                self.escape(doc.span(doc.node(n).value), false)
            }
            Some(ArenaKind::Comment) => {
                if self.comments {
                    put_comment(self.b, doc.span(doc.node(n).value))?;
                }
                Ok(())
            }
            Some(ArenaKind::Pi) => put_pi(self.b, doc, n),
            Some(ArenaKind::DocumentFragment) => self.children(n, depth),
            _ => Ok(()),
        }
    }

    fn children(&mut self, n: NodeId, depth: u32) -> W {
        let doc = self.doc;
        for cid in doc.children(n) {
            self.node(cid, false, depth)?;
        }
        Ok(())
    }

    /// Push `el`'s own xmlns declarations onto the scope.
    fn push_decls(&mut self, el: NodeId) -> W {
        let doc = self.doc;
        for at in doc.attributes(el) {
            if let Some((p, u)) = xmlns_decl(doc, at) {
                self.binds.push(Prefix::Own(p), u)?;
            }
        }
        Ok(())
    }

    /// The declarations the apex renders (§2.2): every prefix in scope at its
    /// innermost binding, since nothing above the apex is output - but not
    /// `xml`, and not a default that undeclares. Read from the scope index,
    /// where it used to walk the ancestors again and check each prefix
    /// against the ones already kept.
    fn apex_namespaces(&self) -> Result<Vec<Ns<'d>>, Failure> {
        let mut out: Vec<Ns> = Vec::new();
        for (prefix, uri) in self.binds.innermost() {
            /* c14n renders the document's own declarations and never invents
             * a prefix, so every binding is one it can borrow. */
            let Prefix::Own(prefix) = *prefix else {
                continue;
            };
            if prefix == b"xml" || (prefix.is_empty() && uri.is_empty()) {
                continue;
            }
            out.falloc_push(Ns { prefix, uri }).or_oom()?;
        }
        sort_by_prefix(&mut out);
        Ok(out)
    }

    /// The declarations of a non-apex `n` that render: those that change what
    /// is in scope (§2.2). Read from the scope BEFORE `n`'s own are pushed, so
    /// a lookup answers what an ancestor declared - one stack lookup, charged
    /// to the step budget, where a walk up the ancestors per declaration made
    /// deep trees quadratic.
    fn own_namespaces(&mut self, n: NodeId) -> Result<Vec<Ns<'d>>, Failure> {
        let doc = self.doc;
        let mut out: Vec<Ns> = Vec::new();
        for at in doc.attributes(n) {
            if let Some((p, u)) = xmlns_decl(doc, at) {
                if p != b"xml" {
                    let above = self.binds.lookup(p)?;
                    let keep = if above == Some(u) {
                        false
                    } else {
                        !(p.is_empty() && u.is_empty()) || above.is_some_and(|a| !a.is_empty())
                    };
                    if keep {
                        out.falloc_push(Ns { prefix: p, uri: u }).or_oom()?;
                    }
                }
            }
        }
        sort_by_prefix(&mut out);
        Ok(out)
    }

    /// The declarations `n` needs and does not hold, pushed onto the scope as
    /// they are found: DOM Level 3's namespace normalization, made where the
    /// canonical form is written. A decided element or attribute (one whose
    /// namespace is its own - `NS_RESOLVED`, or given by `set_attribute_ns`)
    /// whose prefix does not mean its namespace here gets a declaration of that
    /// prefix, on this element. Canonical XML reads the declarations a
    /// document holds, so without it a tree whose namespaces were set rather
    /// than declared - an import from HTML, which the DOM's importNode leaves
    /// with no `xmlns` attribute - could not be canonicalized at all.
    ///
    /// What no declaration here can repair is still refused: a prefix this
    /// element declares for another namespace, one prefix needed for two
    /// ([`Failure::NamespaceMismatch`]); an unprefixed attribute with a
    /// namespace, which only an invented prefix would keep, and inventing one
    /// changes the names in the canonical form (also `NamespaceMismatch`); a
    /// prefix bound to nothing, on a name not decided yet
    /// ([`Failure::UnboundPrefix`]).
    fn fixups(&mut self, n: NodeId) -> Result<Vec<Ns<'d>>, Failure> {
        let doc = self.doc;
        let mut out: Vec<Ns> = Vec::new();
        let decided = doc.node(n).flags.contains(NodeFlags::NS_RESOLVED);
        let el_prefix = doc.span(doc.node(n).prefix);
        if decided {
            self.need(n, &mut out, el_prefix, doc.span(doc.node(n).ns_uri))?;
        } else if self.binds.resolve(el_prefix)?.is_none() {
            /* Undecided, it takes its namespace from the declarations. */
            return Err(Failure::UnboundPrefix);
        }
        for at in doc.attributes(n) {
            /* The XML namespace is always bound to `xml`, and to nothing else
             * (Namespaces in XML §3), so an attribute in it is written as
             * `xml:local` whatever its own prefix (`set_attribute_ns(XML,
             * "a:bb")`), as `to_xml` does. */
            if xmlns_decl(doc, at).is_some() || in_xml_ns(doc, at) {
                continue;
            }
            let prefix = doc.span(doc.node(at).prefix);
            let uri = doc.span(doc.node(at).ns_uri);
            let decided = decided || doc.node(at).attr_ns == crate::xml::AttrNs::Explicit;
            if !prefix.is_empty() {
                if decided {
                    self.need(n, &mut out, prefix, uri)?;
                } else if self.binds.resolve(prefix)?.is_none() {
                    return Err(Failure::UnboundPrefix);
                }
            } else if decided && !uri.is_empty() {
                /* Unprefixed means no namespace (§6.2): only an invented
                 * prefix keeps this one. */
                return Err(Failure::NamespaceMismatch);
            }
        }
        sort_by_prefix(&mut out);
        Ok(out)
    }

    /// That `prefix` means `uri` on `n`: nothing when it already does, else a
    /// declaration of it into `out` and the scope - or the refusal when none
    /// can be made (see [`fixups`](Self::fixups)).
    fn need(&mut self, n: NodeId, out: &mut Vec<Ns<'d>>, prefix: &'d [u8], uri: &'d [u8]) -> W {
        if self.binds.resolve(prefix)? == Some(uri) {
            return Ok(());
        }
        if !prefix.is_empty() && uri.is_empty() {
            /* `xmlns:p=""` is forbidden (§3): a prefix cannot mean none. */
            return Err(Failure::UnboundPrefix);
        }
        let doc = self.doc;
        let declared_here = doc
            .attributes(n)
            .any(|at| doc.decl_prefix(at) == Some(prefix));
        if declared_here || out.iter().any(|f| f.prefix == prefix) {
            return Err(Failure::NamespaceMismatch);
        }
        out.falloc_push(Ns { prefix, uri }).or_oom()?;
        self.binds.push(Prefix::Own(prefix), uri)
    }

    /// The scope for the apex: every ancestor's declarations, outermost first,
    /// since a subtree renders (and is read) under all of them.
    fn push_ancestor_decls(&mut self, n: NodeId) -> W {
        let doc = self.doc;
        let mut chain: Vec<NodeId> = Vec::new();
        let mut up = doc.parent(n);
        while let Some(id) = up {
            if doc.type_(id) == Some(ArenaKind::Element) {
                chain.falloc_push(id).or_oom()?;
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
            return Err(Failure::TooDeep);
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
        /* The apex renders every binding in scope, its own included, so it
         * reads the scope after pushing them; any other element renders what
         * it changes, so it reads the scope before. */
        let rendered = if is_apex {
            self.push_decls(n)?;
            /* The added declarations join the scope, which the apex renders. */
            self.fixups(n)?;
            self.apex_namespaces()?
        } else {
            let mut own = self.own_namespaces(n)?;
            self.push_decls(n)?;
            for f in self.fixups(n)? {
                own.falloc_push(f).or_oom()?;
            }
            sort_by_prefix(&mut own);
            own
        };
        self.put(b"<")?;
        self.qname(n)?;

        for ns in rendered {
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
            if in_xml_ns(doc, at) {
                self.put(b"xml:")?;
                self.put(doc.span(doc.node(at).local))?;
            } else {
                self.qname(at)?;
            }
            self.put(b"=\"")?;
            self.escape(doc.span(doc.node(at).value), true)?;
            self.put(b"\"")?;
        }

        self.put(b">")?;
        for cid in doc.children(n) {
            self.node(cid, false, depth + 1)?;
        }
        self.put(b"</")?;
        self.qname(n)?;
        self.put(b">")
    }
}

/// Whether attribute `at` is in the XML namespace, which is written as `xml:`.
fn in_xml_ns(doc: &XmlDoc, at: NodeId) -> bool {
    doc.span(doc.node(at).ns_uri) == crate::xml::XML_NS_URI
}

/// §3.3: an element's non-declaration attributes, ordered by namespace URI then
/// local name.
fn sorted_attributes(doc: &XmlDoc, n: NodeId) -> Result<Vec<NodeId>, Failure> {
    let mut attrs: Vec<NodeId> = Vec::new();
    for at in doc.attributes(n) {
        if xmlns_decl(doc, at).is_none() {
            attrs.falloc_push(at).or_oom()?;
        }
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
/// lines before and after it. The first failure is the answer: each one stops
/// the walk where it happens, carrying its own reason.
pub(super) fn write(b: &mut Buf, doc: &XmlDoc, n: NodeId, comments: bool) -> W {
    let mut w = Writer {
        b,
        doc,
        comments,
        binds: Bindings::new(),
    };
    if doc.type_(n) != Some(ArenaKind::Document) {
        return w.node(n, true, 0);
    }
    let mut seen_root = false;
    for cid in doc.children(n) {
        let ty = doc.type_(cid);
        if ty == Some(ArenaKind::Element) {
            w.node(cid, true, 0)?;
            seen_root = true;
        } else if ty == Some(ArenaKind::Pi) || (ty == Some(ArenaKind::Comment) && comments) {
            if seen_root {
                w.put(b"\n")?;
            }
            w.node(cid, false, 0)?;
            if !seen_root {
                w.put(b"\n")?;
            }
        }
    }
    Ok(())
}
