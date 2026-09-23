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
//! is now a reverse scan of the bindings actually in scope, and [`NS_STEP_MAX`](super::bindings::NS_STEP_MAX)
//! bounds the total work so a crafted document fails closed rather than hanging.

#![forbid(unsafe_code)]

use super::out::{put, put_pi, W, XML};
use super::Failure;
use crate::cbuf::Buf;
use crate::xml::model::{
    Document as XmlDoc, NodeId, NodeType, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED, MAX_DEPTH,
};
use crate::xml::qname::xmlns_prefix;

use super::bindings::{Bindings, Prefix, PREFIX_CAP};

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
    let resolved = doc.node(el).flags & FLAG_NS_RESOLVED != 0;
    if !resolved && own_decl(doc, el, own_prefix).is_some() {
        /* Not resolved yet (a detached copy or build): its URI reads empty only
         * because nothing has decided it, and its own declaration is what will -
         * so the name and that declaration are written as they are, which is
         * what the element serializes to once inserted. Planning for "no
         * namespace" here gave `xmlns:ns1=""`, which does not parse. */
        plan.declare = false;
    } else if plan.declare && !uri.is_empty() && own_decl(doc, el, own_prefix).is_some() {
        /* The element declares this prefix for a DIFFERENT URI, so its own name
         * needs one of ours. Not for no namespace: no prefix binds to "" -
         * that declaration is ignored instead (`mutate::ignored_default_decl`). */
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
    if is_decl {
        return Some(plan);
    }
    let uri = doc.span(doc.node(a).ns_uri);
    if own_prefix.is_empty() {
        if uri.is_empty() || doc.node(a).flags & FLAG_DOM_LOOSE_NAME != 0 {
            return Some(plan);
        }
        /* An unprefixed attribute is in NO namespace (Namespaces in XML §6.2),
         * so one with a namespace - `set_attribute_ns("urn:p", "c", v)` -
         * written bare lost it on re-parse, and could collide with a plain `c`.
         * It gets a prefix of ours, declared for its URI. */
        plan.prefix = gen_prefix(binds, gen)?;
        plan.declare = true;
        return (!binds.exhausted).then_some(plan);
    }
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
struct Writer<'d, 'b> {
    b: &'b mut Buf,
    doc: &'d XmlDoc,
    /// Spaces per nesting level; 0 for no indentation.
    width: i32,
}

impl<'d, 'b> Writer<'d, 'b> {
    fn new(b: &'b mut Buf, doc: &'d XmlDoc, width: i32) -> Self {
        Writer { b, doc, width }
    }

    fn put(&mut self, bytes: &[u8]) -> W {
        put(self.b, bytes)
    }

    /// The XML declaration a whole-document serialization opens with.
    fn declaration(&mut self, encoding: Option<&[u8]>) -> W {
        if encoding.is_some() || self.doc.has_encoding_decl {
            self.put(b"<?xml version=\"1.0\" encoding=\"")?;
            self.put(encoding.unwrap_or(b"UTF-8"))?;
            return self.put(b"\"?>\n");
        }
        self.put(b"<?xml version=\"1.0\"?>\n")
    }

    fn newline(&mut self) -> W {
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

    /// Declare `prefix` for `uri` and bring it into scope - or refuse, latching
    /// `binds.unbound`, when `prefix` names something and `uri` is empty: a name
    /// whose prefix nothing binds (a detached element's, say) has no
    /// well-formed form, and `xmlns:p=""` was written for it.
    fn bind(&mut self, binds: &mut Bindings<'d>, prefix: Prefix<'d>, uri: &'d [u8]) -> W {
        if !prefix.bytes().is_empty() && uri.is_empty() {
            binds.unbound = true;
            return Err(());
        }
        self.declare(prefix.bytes(), uri)?;
        binds.push(prefix, uri)
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

    /// A CDATA section. Its value can hold `]]>`: the parser merges adjacent
    /// sections into one node (as libxml2 does, for XPath's data model), so
    /// `<![CDATA[a]]]><![CDATA[]>b]]>` is one node reading `a]]>b`. Written raw
    /// that closed the section early and did not re-parse; each `]]>` is split
    /// across two sections instead - `]]` ends one, `>` starts the next - which
    /// is what libxml2 writes, and re-parses (and re-merges) to the same value.
    fn cdata(&mut self, value: &[u8]) -> W {
        self.put(b"<![CDATA[")?;
        let mut rest = value;
        /* A byte scan resumed after each match: linear in the value. (A UTF-8
         * aware search re-validated the rest on every match - quadratic in
         * the number of "]]>".) */
        while let Some(i) = rest.windows(3).position(|w| w == b"]]>") {
            self.put(&rest[..i + 2])?;
            self.put(b"]]><![CDATA[")?;
            rest = &rest[i + 2..];
        }
        self.put(rest)?;
        self.put(b"]]>")
    }

    /// Write `n`. `depth` is both the nesting level the indentation uses and the
    /// recursion guard - they were two arguments carrying the same number.
    fn node(&mut self, n: NodeId, depth: u32, binds: &mut Bindings<'d>) -> W {
        let doc = self.doc;
        match doc.type_(n) {
            Some(NodeType::Doctype) => self.doctype(n),
            Some(NodeType::Element) => self.element(n, depth, binds),
            Some(NodeType::Text) => self.escape(doc.span(doc.node(n).value), false),
            Some(NodeType::CData) => self.cdata(doc.span(doc.node(n).value)),
            Some(NodeType::Comment) => {
                self.put(b"<!--")?;
                self.put(doc.span(doc.node(n).value))?;
                self.put(b"-->")
            }
            Some(NodeType::Pi) => put_pi(self.b, doc, n),
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

    fn doctype(&mut self, dt: NodeId) -> W {
        let doc = self.doc;
        self.put(b"<!DOCTYPE ")?;
        self.put(doc.span(doc.node(dt).local))?;
        let (prefix, value) = (doc.node(dt).prefix, doc.node(dt).value);
        if !prefix.is_absent() {
            self.put(b" PUBLIC ")?;
            self.literal(doc.span(prefix))?;
            self.put(b" ")?;
            self.literal(doc.span(value))?;
        } else if !value.is_absent() {
            self.put(b" SYSTEM ")?;
            self.literal(doc.span(value))?;
        }
        self.put(b">")
    }

    /// A doctype id as a quoted literal. The grammar has no escape: a literal
    /// is `"..."` or `'...'` and holds no instance of its own quote, and a
    /// single-quoted SYSTEM id may contain `"` - always writing `"` made that
    /// one ill-formed. (A PUBLIC id's characters exclude `"`, so it is always
    /// double-quoted, as before.)
    fn literal(&mut self, id: &[u8]) -> W {
        let quote: &[u8] = if id.contains(&b'"') { b"'" } else { b"\"" };
        self.put(quote)?;
        self.put(id)?;
        self.put(quote)
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
        let dropped = crate::xml::mutate::ignored_default_decl(doc, n);

        /* This element's own xmlns declarations bind from here down. */
        let mut a = doc.attrs(n);
        while let Some(at) = a {
            if Some(at) == dropped {
                a = doc.next(at);
                continue;
            }
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
            self.bind(binds, el.prefix.clone(), doc.span(doc.node(n).ns_uri))?;
        }

        let mut a = doc.attrs(n);
        while let Some(at) = a {
            if Some(at) == dropped {
                a = doc.next(at);
                continue;
            }
            let plan = plan_attr(doc, n, at, binds, &mut gen).ok_or(())?;
            if plan.declare {
                self.bind(binds, plan.prefix.clone(), doc.span(doc.node(at).ns_uri))?;
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

/// Write `n` as XML 1.0 into `b`.
///
/// The whole of this module's surface: the scope stack, the writer and the
/// document-level layout stay inside, so `super` picks a form and maps a failure
/// and knows nothing of how either is shaped.
pub(super) fn write(
    b: &mut Buf,
    doc: &XmlDoc,
    n: NodeId,
    indent: i32,
    encoding: Option<&[u8]>,
) -> Result<(), Failure> {
    let mut binds = Bindings::new();
    let r = (|| -> W {
        let mut w = Writer::new(b, doc, indent);
        if doc.type_(n) != Some(NodeType::Document) {
            return w.node(n, 0, &mut binds);
        }
        /* The Document node gives the declaration, then each top-level child on
         * its own line. */
        w.declaration(encoding)?;
        let mut c = doc.first_child(n);
        while let Some(cid) = c {
            w.node(cid, 0, &mut binds)?;
            w.newline()?;
            c = doc.next(cid);
        }
        Ok(())
    })();
    match r {
        Ok(()) => Ok(()),
        /* Reading the flag after the fact is exact, not a guess: only a lookup
         * sets it, and a planner that sees it set refuses immediately - so a
         * write failure always propagates with the flag still clear. */
        Err(()) if binds.exhausted => Err(Failure::NamespaceBudget),
        Err(()) if binds.unbound => Err(Failure::UnboundPrefix),
        Err(()) => Err(Failure::Output),
    }
}
