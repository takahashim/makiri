//! XML 1.0 output, and the namespace planner that keeps it well-formed.
//!
//! # The bindings are a stack, not a chain
//!
//! An element's namespace URI is its IDENTITY (`FLAG_NS_RESOLVED`), not
//! something read off the declarations around it, so the writer must emit
//! whatever declarations the output needs to reproduce each URI - inventing a
//! prefix when the natural one is taken. Deciding that needs the bindings in
//! scope, and they live in ONE stack, exactly as the parser keeps them
//! (`tree::Parser::binds` plus its frame bases).
//!
//! That is a fix, not a style: the bindings used to be a chain of per-element
//! links, and resolving a prefix walked it re-scanning every ancestor's
//! ATTRIBUTE LIST. `to_xml` therefore cost O(depth^2 x attributes) - a 403 KB
//! document of 800 nested elements carrying 50 prefixed attributes each took
//! 4.88s, against 0.097s for the same byte count without prefixes. A resolution
//! is now a reverse scan of the bindings actually in scope, and [`NS_STEP_MAX`]
//! bounds the total work so a crafted document fails closed rather than hanging.

#![forbid(unsafe_code)]

use super::out::{put, W, XML};
use super::Failure;
use crate::cbuf::Buf;
use crate::falloc::Reserve;
use crate::xml::model::{Document as XmlDoc, NodeId, NodeType, FLAG_DOM_LOOSE_NAME, MAX_DEPTH};
use crate::xml::qname::xmlns_prefix;

/// The total prefix-resolution steps one serialization may take. Generous - an
/// ordinary document uses a handful - but finite, so namespace planning cannot
/// be made to run unboundedly long by nesting and prefix count alone.
const NS_STEP_MAX: u64 = 64 * 1024 * 1024;

const PREFIX_CAP: usize = 8;

/// A namespace prefix: borrowed from the arena when the document supplied it,
/// owned inline when the serializer invented it.
#[derive(Clone)]
enum Prefix<'d> {
    Own(&'d [u8]),
    Invented([u8; PREFIX_CAP], usize),
}

impl Prefix<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Prefix::Own(s) => s,
            Prefix::Invented(b, n) => &b[..*n],
        }
    }
    fn is_invented(&self) -> bool {
        matches!(self, Prefix::Invented(..))
    }
}

/// Every binding in scope at the current element, innermost last.
///
/// One element's entries are its own xmlns declarations plus at most one
/// declaration the planner synthesized for the element's own name; they are
/// pushed on entry and truncated away on exit. Within one element the order is
/// immaterial: a prefix is declared at most once per element (the parser rejects
/// a duplicate and the DOM replaces it), and an invented prefix is chosen
/// unbound, so no two entries from the same element share a prefix.
pub(super) struct Bindings<'d> {
    stack: Vec<(Prefix<'d>, &'d [u8])>,
    steps: u64,
    /// Latched once the step budget is spent. A lookup then answers `None`,
    /// which every caller turns into a refusal, so an exhausted planner can
    /// never emit a declaration it did not verify.
    exhausted: bool,
}

impl<'d> Bindings<'d> {
    fn new() -> Self {
        Bindings {
            stack: Vec::new(),
            steps: 0,
            exhausted: false,
        }
    }

    fn len(&self) -> usize {
        self.stack.len()
    }

    fn truncate(&mut self, base: usize) {
        self.stack.truncate(base);
    }

    fn push(&mut self, prefix: Prefix<'d>, uri: &'d [u8]) -> W {
        self.stack.falloc_reserve(1).map_err(|_| ())?;
        self.stack.push((prefix, uri));
        Ok(())
    }

    /// The innermost binding for `prefix`, or None when it is unbound - or when
    /// the step budget ran out, which [`Bindings::exhausted`] then reports.
    fn lookup(&mut self, prefix: &[u8]) -> Option<&'d [u8]> {
        for (p, uri) in self.stack.iter().rev() {
            self.steps += 1;
            if self.steps > NS_STEP_MAX {
                self.exhausted = true;
                return None;
            }
            if p.bytes() == prefix {
                return Some(uri);
            }
        }
        None
    }

    /// `xml` is bound everywhere and never declared (Namespaces in XML §3).
    fn bound_to(&mut self, prefix: &[u8], uri: &[u8]) -> bool {
        if prefix == b"xml" {
            return true;
        }
        match self.lookup(prefix) {
            None => prefix.is_empty() && uri.is_empty(),
            Some(got) => got == uri,
        }
    }

    fn is_bound(&mut self, prefix: &[u8]) -> bool {
        prefix == b"xml" || self.lookup(prefix).is_some()
    }
}

/* ---- planning one name ---- */

/// The declaration for `prefix` on `el` ITSELF (not in scope), or None.
fn own_decl(doc: &XmlDoc, el: NodeId, prefix: &[u8]) -> Option<NodeId> {
    let mut a = doc.attrs(el);
    while let Some(at) = a {
        if let Some(p) = xmlns_prefix(doc.qname(at)) {
            if p == prefix {
                return Some(at);
            }
        }
        a = doc.next(at);
    }
    None
}

/// The first attribute of `el` before `stop` that carries `prefix`, or None.
fn prefix_seen(doc: &XmlDoc, el: NodeId, stop: NodeId, prefix: &[u8]) -> Option<NodeId> {
    let mut a = doc.attrs(el);
    while let Some(at) = a {
        if at == stop {
            break;
        }
        if doc.node(at).prefix.len != 0
            && xmlns_prefix(doc.qname(at)).is_none()
            && doc.prefix(at) == prefix
        {
            return Some(at);
        }
        a = doc.next(at);
    }
    None
}

struct Gen {
    seq: u32,
}

fn gen_prefix<'d>(binds: &mut Bindings<'d>, gen: &mut Gen) -> Option<Prefix<'d>> {
    const GEN_MAX: u32 = 100_000;
    while gen.seq < GEN_MAX {
        let mut buf = [0u8; PREFIX_CAP];
        let mut i = 0usize;
        buf[i] = b'n';
        i += 1;
        buf[i] = b's';
        i += 1;
        let (mut div, mut started) = (10_000u32, false);
        while div > 0 {
            let d = (gen.seq / div) % 10;
            if d != 0 || started || div == 1 {
                buf[i] = b'0' + d as u8;
                i += 1;
                started = true;
            }
            div /= 10;
        }
        gen.seq += 1;
        if !binds.is_bound(&buf[..i]) {
            return Some(Prefix::Invented(buf, i));
        }
    }
    None
}

/// How one name will be written: under which prefix, and whether the output has
/// to declare it here.
struct Plan<'d> {
    prefix: Prefix<'d>,
    declare: bool,
}

impl Plan<'_> {
    fn bytes(&self) -> &[u8] {
        self.prefix.bytes()
    }
    fn renamed(&self) -> bool {
        self.prefix.is_invented()
    }
}

/// The plan for `el`'s own name. `None` refuses the document: either no prefix
/// could be invented or the step budget is spent.
fn plan_element<'d>(
    doc: &'d XmlDoc,
    el: NodeId,
    binds: &mut Bindings<'d>,
    gen: &mut Gen,
) -> Option<Plan<'d>> {
    let own_prefix = doc.span(doc.node(el).prefix);
    let uri = doc.span(doc.node(el).ns_uri);
    let mut plan = Plan {
        prefix: Prefix::Own(own_prefix),
        declare: doc.node(el).flags & FLAG_DOM_LOOSE_NAME == 0 && !binds.bound_to(own_prefix, uri),
    };
    if plan.declare && own_decl(doc, el, own_prefix).is_some() {
        /* The element declares this prefix for a DIFFERENT URI, so its own name
         * needs one of ours. */
        plan.prefix = gen_prefix(binds, gen)?;
    }
    (!binds.exhausted).then_some(plan)
}

/// The plan for attribute `a` of `el`.
fn plan_attr<'d>(
    doc: &'d XmlDoc,
    el: NodeId,
    a: NodeId,
    binds: &mut Bindings<'d>,
    gen: &mut Gen,
) -> Option<Plan<'d>> {
    let own_prefix = doc.span(doc.node(a).prefix);
    let mut plan = Plan {
        prefix: Prefix::Own(own_prefix),
        declare: false,
    };

    let is_decl = xmlns_prefix(doc.qname(a)).is_some();
    if own_prefix.is_empty() || is_decl {
        return Some(plan);
    }
    let uri = doc.span(doc.node(a).ns_uri);
    if binds.bound_to(own_prefix, uri) {
        return (!binds.exhausted).then_some(plan);
    }

    let prior = prefix_seen(doc, el, a, own_prefix);
    let taken = binds.is_bound(own_prefix);
    if !taken {
        if let Some(p) = prior {
            if doc.span(doc.node(p).ns_uri) == uri {
                return (!binds.exhausted).then_some(plan);
            }
        }
    }
    if taken || prior.is_some() {
        plan.prefix = gen_prefix(binds, gen)?;
    }
    plan.declare = true;
    (!binds.exhausted).then_some(plan)
}

/* ---- writing ---- */

/// The XML 1.0 writer: the output buffer, the document it reads, and the indent
/// width, so only what actually varies per node travels as an argument.
pub(super) struct Writer<'d, 'b> {
    b: &'b mut Buf,
    doc: &'d XmlDoc,
    /// Spaces per nesting level; 0 for no indentation.
    width: i32,
}

impl<'d, 'b> Writer<'d, 'b> {
    pub(super) fn new(b: &'b mut Buf, doc: &'d XmlDoc, width: i32) -> Self {
        Writer { b, doc, width }
    }

    fn put(&mut self, bytes: &[u8]) -> W {
        put(self.b, bytes)
    }

    /// The XML declaration a whole-document serialization opens with.
    pub(super) fn declaration(&mut self, encoding: Option<&[u8]>) -> W {
        if encoding.is_some() || self.doc.has_encoding_decl {
            self.put(b"<?xml version=\"1.0\" encoding=\"")?;
            self.put(encoding.unwrap_or(b"UTF-8"))?;
            return self.put(b"\"?>\n");
        }
        self.put(b"<?xml version=\"1.0\"?>\n")
    }

    pub(super) fn newline(&mut self) -> W {
        self.put(b"\n")
    }

    fn escape(&mut self, s: &[u8], attr: bool) -> W {
        XML.write(self.b, s, attr)
    }

    fn indent(&mut self, level: u32) -> W {
        self.put(b"\n")?;
        for _ in 0..(level as i32).saturating_mul(self.width) {
            self.put(b" ")?;
        }
        Ok(())
    }

    /// `n`'s own name, under the plan's prefix.
    fn name(&mut self, n: NodeId, plan: &Plan) -> W {
        let doc = self.doc;
        if !plan.renamed() {
            return self.put(doc.span(doc.node(n).qname));
        }
        self.put(plan.bytes())?;
        self.put(b":")?;
        self.put(doc.span(doc.node(n).local))
    }

    fn declare(&mut self, prefix: &[u8], uri: &'d [u8]) -> W {
        self.put(b" xmlns")?;
        if !prefix.is_empty() {
            self.put(b":")?;
            self.put(prefix)?;
        }
        self.put(b"=\"")?;
        self.escape(uri, true)?;
        self.put(b"\"")
    }

    /// Write `n`. `depth` is both the nesting level the indentation uses and the
    /// recursion guard - they were two arguments carrying the same number.
    pub(super) fn node(&mut self, n: NodeId, depth: u32, binds: &mut Bindings<'d>) -> W {
        let doc = self.doc;
        match doc.type_(n) {
            Some(NodeType::Doctype) => self.doctype(n),
            Some(NodeType::Element) => self.element(n, depth, binds),
            Some(NodeType::Text) => self.escape(doc.span(doc.node(n).value), false),
            Some(NodeType::CData) => {
                self.put(b"<![CDATA[")?;
                self.put(doc.span(doc.node(n).value))?;
                self.put(b"]]>")
            }
            Some(NodeType::Comment) => {
                self.put(b"<!--")?;
                self.put(doc.span(doc.node(n).value))?;
                self.put(b"-->")
            }
            Some(NodeType::Pi) => self.pi(n),
            Some(NodeType::Fragment) => {
                let mut c = doc.first_child(n);
                while let Some(cid) = c {
                    self.node(cid, depth, binds)?;
                    c = doc.next(cid);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn pi(&mut self, n: NodeId) -> W {
        let doc = self.doc;
        self.put(b"<?")?;
        self.put(doc.span(doc.node(n).local))?;
        if doc.node(n).value.len != 0 {
            self.put(b" ")?;
            self.put(doc.span(doc.node(n).value))?;
        }
        self.put(b"?>")
    }

    fn doctype(&mut self, dt: NodeId) -> W {
        let doc = self.doc;
        self.put(b"<!DOCTYPE ")?;
        self.put(doc.span(doc.node(dt).local))?;
        let (prefix, value) = (doc.node(dt).prefix, doc.node(dt).value);
        if !prefix.is_absent() {
            self.put(b" PUBLIC \"")?;
            self.put(doc.span(prefix))?;
            self.put(b"\" \"")?;
            self.put(doc.span(value))?;
            self.put(b"\"")?;
        } else if !value.is_absent() {
            self.put(b" SYSTEM \"")?;
            self.put(doc.span(value))?;
            self.put(b"\"")?;
        }
        self.put(b">")
    }

    fn element(&mut self, n: NodeId, depth: u32, binds: &mut Bindings<'d>) -> W {
        if depth as usize >= MAX_DEPTH {
            return Err(());
        }
        let base = binds.len();
        let r = self.element_in_scope(n, depth, binds);
        binds.truncate(base);
        r
    }

    /// The element's own bindings are pushed for the duration; [`element`] owns
    /// popping them, so an early `?` cannot leave the scope stack unbalanced.
    fn element_in_scope(&mut self, n: NodeId, depth: u32, binds: &mut Bindings<'d>) -> W {
        let doc = self.doc;

        /* This element's own xmlns declarations bind from here down. */
        let mut a = doc.attrs(n);
        while let Some(at) = a {
            if let Some(p) = xmlns_prefix(doc.qname(at)) {
                binds.push(Prefix::Own(p), doc.span(doc.node(at).value))?;
            }
            a = doc.next(at);
        }

        let mut gen = Gen { seq: 1 };
        let el = plan_element(doc, n, binds, &mut gen).ok_or(())?;

        self.put(b"<")?;
        self.name(n, &el)?;
        if el.declare {
            let uri = doc.span(doc.node(n).ns_uri);
            let prefix = el.prefix.clone();
            self.declare(prefix.bytes(), uri)?;
            binds.push(prefix, uri)?;
        }

        let mut a = doc.attrs(n);
        while let Some(at) = a {
            let plan = plan_attr(doc, n, at, binds, &mut gen).ok_or(())?;
            if plan.declare {
                let uri = doc.span(doc.node(at).ns_uri);
                let prefix = plan.prefix.clone();
                self.declare(prefix.bytes(), uri)?;
                binds.push(prefix, uri)?;
            }
            self.put(b" ")?;
            self.name(at, &plan)?;
            self.put(b"=\"")?;
            self.escape(doc.span(doc.node(at).value), true)?;
            self.put(b"\"")?;
            a = doc.next(at);
        }

        if doc.first_child(n).is_none() {
            return self.put(b"/>");
        }
        self.put(b">")?;
        let block = self.width > 0 && !has_chardata(doc, n);
        let mut c = doc.first_child(n);
        while let Some(cid) = c {
            if block {
                self.indent(depth + 1)?;
            }
            self.node(cid, depth + 1, binds)?;
            c = doc.next(cid);
        }
        if block {
            self.indent(depth)?;
        }
        self.put(b"</")?;
        self.name(n, &el)?;
        self.put(b">")
    }
}

fn has_chardata(doc: &XmlDoc, e: NodeId) -> bool {
    let mut c = doc.first_child(e);
    while let Some(id) = c {
        if matches!(doc.type_(id), Some(NodeType::Text | NodeType::CData)) {
            return true;
        }
        c = doc.next(id);
    }
    false
}

/// Run `f` with a fresh, empty scope. The only way to obtain a [`Bindings`],
/// so a writer cannot start with someone else's scope.
pub(super) fn with_scope<'d>(f: impl FnOnce(&mut Bindings<'d>) -> W) -> Result<(), Failure> {
    let mut binds = Bindings::new();
    let r = f(&mut binds);
    match r {
        Ok(()) => Ok(()),
        Err(()) if binds.exhausted => Err(Failure::NamespaceBudget),
        Err(()) => Err(Failure::Output),
    }
}
