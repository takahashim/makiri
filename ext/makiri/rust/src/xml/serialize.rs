//! Turning an XML node back into text: XML 1.0 and Inclusive Canonical XML 1.0.
//!
//! Ruby-free, like the rest of `xml`. The Ruby methods (`#to_xml`, `#canonicalize`)
//! live in `glue::xml_node::serialize`, which parses their options, turns the
//! bytes into a String and maps a [`Failure`] to its exception.
//!
//! Output is always well-formed and re-parses to the same tree. xmlns
//! declarations ride along as ordinary attribute nodes, so namespaces
//! round-trip.
//!
//! # The scope chain owns its prefixes
//!
//! The namespace planner threads a chain of bindings down a recursion, and a
//! link can hold a prefix the serializer INVENTED - which a descendant then
//! reads. A prefix is a [`Prefix`], which owns its bytes inline when they were
//! invented and borrows the arena when they were not.
//!
//! # Reading the arena
//!
//! The node is an index-arena `NodeId` and its bytes live in the document, so
//! these readers carry the document. [`field`] takes a span and extends the
//! borrow to `'static`: the public entry points borrow the document for the
//! whole call, so nothing can drop or mutate it underneath.

use crate::falloc::Reserve;
use core::ffi::c_void;

use crate::cbuf::{mkr_buf_append, Buf, MKR_OK};
use crate::xml::model::{Doc as XmlDoc, NodeId, NodeType, Span, FLAG_DOM_LOOSE_NAME, MAX_DEPTH};

/// Why serialization produced no output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A DOM-loose element name - created through the browser-DOM interop
    /// hatch - has no XML form.
    DomLooseName,
    /// The output exceeded its ceiling, or memory ran out.
    Output,
}

/// The output ceiling for `doc`: generous, but proportional to its arena, so a
/// namespace-heavy tree cannot expand without bound.
pub fn output_cap(doc: &XmlDoc) -> usize {
    65536usize.saturating_add(doc.arena_bytes.saturating_mul(32))
}

/// `n` as XML 1.0, indented by `indent` spaces per level (0 for none).
///
/// The Document node gives the XML declaration and then each top-level child
/// on its own line. The declaration names `encoding` when given, and otherwise
/// UTF-8 when the parsed document declared an encoding at all.
pub fn to_xml(
    doc: &XmlDoc,
    n: NodeId,
    indent: i32,
    encoding: Option<&[u8]>,
) -> Result<Buf, Failure> {
    // SAFETY: `doc` is borrowed for the whole call, which is what `field`'s
    // lifetime extension and the scope chain rely on; `b` points at the local
    // buffer and is not used after it.
    unsafe {
        if has_dom_loose_name(doc, n) {
            return Err(Failure::DomLooseName);
        }
        let mut buf = Buf::new(output_cap(doc));
        let b = &mut buf as *mut Buf;
        let rc = (|| -> W {
            if doc.type_(n) != Some(NodeType::Document) {
                return write_node(b, doc, n, 0, indent, None, 0);
            }
            if encoding.is_some() || doc.has_encoding_decl {
                put(b, b"<?xml version=\"1.0\" encoding=\"")?;
                put(b, encoding.unwrap_or(b"UTF-8"))?;
                put(b, b"\"?>\n")?;
            } else {
                put(b, b"<?xml version=\"1.0\"?>\n")?;
            }
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                write_node(b, doc, cid, 0, indent, None, 0)?;
                put(b, b"\n")?;
                c = doc.next(cid);
            }
            Ok(())
        })();
        rc.map(|()| buf).map_err(|()| Failure::Output)
    }
}

/// `n` as Inclusive Canonical XML 1.0, with or without comments.
///
/// For the Document node that is the root element, plus the top-level PIs (and
/// comments, when asked for) on their own lines before and after it.
pub fn canonicalize(doc: &XmlDoc, n: NodeId, comments: bool) -> Result<Buf, Failure> {
    // SAFETY: as in `to_xml`.
    unsafe {
        if has_dom_loose_name(doc, n) {
            return Err(Failure::DomLooseName);
        }
        let mut buf = Buf::new(output_cap(doc));
        let b = &mut buf as *mut Buf;
        let rc = (|| -> W {
            if doc.type_(n) != Some(NodeType::Document) {
                return c14n_node(b, doc, n, true, comments, 0);
            }
            let mut seen_root = false;
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                let ty = doc.type_(cid);
                if ty == Some(NodeType::Element) {
                    c14n_node(b, doc, cid, true, comments, 0)?;
                    seen_root = true;
                } else if ty == Some(NodeType::Pi) || (ty == Some(NodeType::Comment) && comments) {
                    if seen_root {
                        put(b, b"\n")?;
                    }
                    c14n_node(b, doc, cid, false, comments, 0)?;
                    if !seen_root {
                        put(b, b"\n")?;
                    }
                }
                c = doc.next(cid);
            }
            Ok(())
        })();
        rc.map(|()| buf).map_err(|()| Failure::Output)
    }
}

/* ------------------------------------------------------------------ */
/* the output buffer                                                  */
/* ------------------------------------------------------------------ */

type W = Result<(), ()>;

unsafe fn put(b: *mut Buf, bytes: &[u8]) -> W {
    if bytes.is_empty() {
        return Ok(());
    }
    if mkr_buf_append(b, bytes.as_ptr() as *const c_void, bytes.len()) == MKR_OK {
        Ok(())
    } else {
        Err(())
    }
}

/// A node span as a slice with a `'static` lifetime claim. The caller must keep
/// the document alive for the reference's use; serialization does, for the whole
/// call.
unsafe fn field<'a>(doc: &XmlDoc, s: Span) -> &'a [u8] {
    core::mem::transmute::<&[u8], &'a [u8]>(doc.span(s))
}

/* ------------------------------------------------------------------ */
/* XML escaping                                                       */
/* ------------------------------------------------------------------ */

unsafe fn escaped(b: *mut Buf, s: &[u8], attr: bool) -> W {
    let mut start = 0usize;
    for (i, &c) in s.iter().enumerate() {
        let rep: &[u8] = match c {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' if attr => b"&quot;",
            b'\t' if attr => b"&#9;",
            b'\n' if attr => b"&#10;",
            b'\r' => b"&#13;",
            _ => continue,
        };
        if i > start {
            put(b, &s[start..i])?;
        }
        put(b, rep)?;
        start = i + 1;
    }
    if s.len() > start {
        put(b, &s[start..])?;
    }
    Ok(())
}

/* ------------------------------------------------------------------ */
/* namespace declarations                                             */
/* ------------------------------------------------------------------ */

const PREFIX_CAP: usize = 8;

#[derive(Clone)]
enum Prefix {
    Own(&'static [u8]),
    Invented([u8; PREFIX_CAP], usize),
}

impl Prefix {
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

struct Scope<'a> {
    up: Option<&'a Scope<'a>>,
    doc: &'a XmlDoc,
    /// Its xmlns attributes bind at this level.
    el: NodeId,
    /// The declaration synthesized for this element's own name, if any.
    syn: Option<(Prefix, &'static [u8])>,
}

/// The declaration for `prefix` on `el` itself, or None.
unsafe fn own_decl(doc: &XmlDoc, el: NodeId, prefix: &[u8]) -> Option<NodeId> {
    let mut a = doc.attrs(el);
    while let Some(at) = a {
        if let Some(p) = crate::xml::qname::xmlns_prefix(doc.qname(at)) {
            if p == prefix {
                return Some(at);
            }
        }
        a = doc.next(at);
    }
    None
}

unsafe fn lookup<'a>(scope: Option<&'a Scope<'a>>, prefix: &[u8]) -> Option<&'a [u8]> {
    let mut s = scope;
    while let Some(cur) = s {
        if let Some(d) = own_decl(cur.doc, cur.el, prefix) {
            return Some(field(cur.doc, cur.doc.node(d).value));
        }
        if let Some((p, uri)) = cur.syn.as_ref() {
            if p.bytes() == prefix {
                return Some(uri);
            }
        }
        s = cur.up;
    }
    None
}

fn is_xml_prefix(prefix: &[u8]) -> bool {
    prefix == b"xml"
}

unsafe fn bound_to(scope: Option<&Scope>, prefix: &[u8], uri: &[u8]) -> bool {
    if is_xml_prefix(prefix) {
        return true;
    }
    match lookup(scope, prefix) {
        None => prefix.is_empty() && uri.is_empty(),
        Some(got) => got == uri,
    }
}

unsafe fn is_bound(scope: Option<&Scope>, prefix: &[u8]) -> bool {
    is_xml_prefix(prefix) || lookup(scope, prefix).is_some()
}

unsafe fn declare(b: *mut Buf, prefix: &[u8], uri: &[u8]) -> W {
    put(b, b" xmlns")?;
    if !prefix.is_empty() {
        put(b, b":")?;
        put(b, prefix)?;
    }
    put(b, b"=\"")?;
    escaped(b, uri, true)?;
    put(b, b"\"")
}

struct Gen {
    seq: u32,
}

unsafe fn gen_prefix(scope: Option<&Scope>, gen: &mut Gen) -> Option<Prefix> {
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
        if !is_bound(scope, &buf[..i]) {
            return Some(Prefix::Invented(buf, i));
        }
    }
    None
}

/// The first attribute of `el` before `stop` that carries `prefix`, or None.
unsafe fn prefix_seen(doc: &XmlDoc, el: NodeId, stop: NodeId, prefix: &[u8]) -> Option<NodeId> {
    let mut a = doc.attrs(el);
    while let Some(at) = a {
        if at == stop {
            break;
        }
        if doc.node(at).prefix.len != 0
            && crate::xml::qname::xmlns_prefix(doc.qname(at)).is_none()
            && doc.prefix(at) == prefix
        {
            return Some(at);
        }
        a = doc.next(at);
    }
    None
}

struct Plan {
    prefix: Prefix,
    declare: bool,
}

impl Plan {
    fn bytes(&self) -> &[u8] {
        self.prefix.bytes()
    }
    fn renamed(&self) -> bool {
        self.prefix.is_invented()
    }
}

unsafe fn plan_element(here: &Scope, gen: &mut Gen) -> Option<Plan> {
    let doc = here.doc;
    let n = here.el;
    let own_prefix = field(doc, doc.node(n).prefix);
    let mut plan = Plan {
        prefix: Prefix::Own(own_prefix),
        declare: doc.node(n).flags & FLAG_DOM_LOOSE_NAME == 0
            && !bound_to(Some(here), own_prefix, field(doc, doc.node(n).ns_uri)),
    };
    if plan.declare && own_decl(doc, n, own_prefix).is_some() {
        plan.prefix = gen_prefix(Some(here), gen)?;
    }
    Some(plan)
}

unsafe fn plan_attr(here: &Scope, a: NodeId, gen: &mut Gen) -> Option<Plan> {
    let doc = here.doc;
    let own_prefix = field(doc, doc.node(a).prefix);
    let mut plan = Plan {
        prefix: Prefix::Own(own_prefix),
        declare: false,
    };

    let is_decl = crate::xml::qname::xmlns_prefix(doc.qname(a)).is_some();
    if own_prefix.is_empty() || is_decl {
        return Some(plan);
    }
    let uri = field(doc, doc.node(a).ns_uri);
    if bound_to(Some(here), own_prefix, uri) {
        return Some(plan);
    }

    let prior = prefix_seen(doc, here.el, a, own_prefix);
    let taken = is_bound(Some(here), own_prefix);
    if !taken {
        if let Some(p) = prior {
            if field(doc, doc.node(p).ns_uri) == uri {
                return Some(plan);
            }
        }
    }
    if taken || prior.is_some() {
        plan.prefix = gen_prefix(Some(here), gen)?;
    }
    plan.declare = true;
    Some(plan)
}

unsafe fn write_name(b: *mut Buf, doc: &XmlDoc, n: NodeId, plan: &Plan) -> W {
    if !plan.renamed() {
        return put(b, field(doc, doc.node(n).qname));
    }
    put(b, plan.bytes())?;
    put(b, b":")?;
    put(b, field(doc, doc.node(n).local))
}

unsafe fn indent(b: *mut Buf, level: i32, width: i32) -> W {
    put(b, b"\n")?;
    for _ in 0..level * width {
        put(b, b" ")?;
    }
    Ok(())
}

unsafe fn has_chardata(doc: &XmlDoc, e: NodeId) -> bool {
    let mut c = doc.first_child(e);
    while let Some(id) = c {
        if matches!(doc.type_(id), Some(NodeType::Text | NodeType::CData)) {
            return true;
        }
        c = doc.next(id);
    }
    false
}

unsafe fn has_dom_loose_name(doc: &XmlDoc, root: NodeId) -> bool {
    let mut cur = Some(root);
    while let Some(id) = cur {
        if doc.type_(id) == Some(NodeType::Element) && doc.node(id).flags & FLAG_DOM_LOOSE_NAME != 0
        {
            return true;
        }
        cur = doc.preorder_next(root, id);
    }
    false
}

unsafe fn write_doctype(b: *mut Buf, doc: &XmlDoc, dt: NodeId) -> W {
    put(b, b"<!DOCTYPE ")?;
    put(b, field(doc, doc.node(dt).local))?;
    let prefix = doc.node(dt).prefix;
    let value = doc.node(dt).value;
    if !prefix.is_absent() {
        put(b, b" PUBLIC \"")?;
        put(b, field(doc, prefix))?;
        put(b, b"\" \"")?;
        put(b, field(doc, value))?;
        put(b, b"\"")?;
    } else if !value.is_absent() {
        put(b, b" SYSTEM \"")?;
        put(b, field(doc, value))?;
        put(b, b"\"")?;
    }
    put(b, b">")
}

unsafe fn write_node<'a>(
    b: *mut Buf,
    doc: &'a XmlDoc,
    n: NodeId,
    level: i32,
    width: i32,
    scope: Option<&'a Scope<'a>>,
    depth: u32,
) -> W {
    match doc.type_(n) {
        Some(NodeType::Doctype) => write_doctype(b, doc, n),
        Some(NodeType::Element) => {
            if depth as usize >= MAX_DEPTH {
                return Err(());
            }
            let mut here = Scope {
                up: scope,
                doc,
                el: n,
                syn: None,
            };
            let mut gen = Gen { seq: 1 };

            let el = plan_element(&here, &mut gen).ok_or(())?;

            put(b, b"<")?;
            write_name(b, doc, n, &el)?;
            if el.declare {
                declare(b, el.bytes(), field(doc, doc.node(n).ns_uri))?;
                here.syn = Some((el.prefix.clone(), field(doc, doc.node(n).ns_uri)));
            }

            let mut a = doc.attrs(n);
            while let Some(at) = a {
                let plan = plan_attr(&here, at, &mut gen).ok_or(())?;
                if plan.declare {
                    declare(b, plan.bytes(), field(doc, doc.node(at).ns_uri))?;
                }
                put(b, b" ")?;
                write_name(b, doc, at, &plan)?;
                put(b, b"=\"")?;
                escaped(b, field(doc, doc.node(at).value), true)?;
                put(b, b"\"")?;
                a = doc.next(at);
            }

            if doc.first_child(n).is_none() {
                return put(b, b"/>");
            }
            put(b, b">")?;
            let block = width > 0 && !has_chardata(doc, n);
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                if block {
                    indent(b, level + 1, width)?;
                }
                write_node(b, doc, cid, level + 1, width, Some(&here), depth + 1)?;
                c = doc.next(cid);
            }
            if block {
                indent(b, level, width)?;
            }
            put(b, b"</")?;
            write_name(b, doc, n, &el)?;
            put(b, b">")
        }
        Some(NodeType::Text) => escaped(b, field(doc, doc.node(n).value), false),
        Some(NodeType::CData) => {
            put(b, b"<![CDATA[")?;
            put(b, field(doc, doc.node(n).value))?;
            put(b, b"]]>")
        }
        Some(NodeType::Comment) => {
            put(b, b"<!--")?;
            put(b, field(doc, doc.node(n).value))?;
            put(b, b"-->")
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
                write_node(b, doc, cid, level, width, scope, depth)?;
                c = doc.next(cid);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/* ------------------------------------------------------------------ */
/* Canonical XML 1.0                                                  */
/* ------------------------------------------------------------------ */

unsafe fn c14n_escaped(b: *mut Buf, s: &[u8], attr: bool) -> W {
    let mut start = 0usize;
    for (i, &c) in s.iter().enumerate() {
        let rep: &[u8] = match c {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' if !attr => b"&gt;",
            b'"' if attr => b"&quot;",
            b'\t' if attr => b"&#x9;",
            b'\n' if attr => b"&#xA;",
            b'\r' => b"&#xD;",
            _ => continue,
        };
        if i > start {
            put(b, &s[start..i])?;
        }
        put(b, rep)?;
        start = i + 1;
    }
    if s.len() > start {
        put(b, &s[start..])?;
    }
    Ok(())
}

unsafe fn xmlns_decl(doc: &XmlDoc, a: NodeId) -> Option<(&'static [u8], &'static [u8])> {
    let p = crate::xml::qname::xmlns_prefix(doc.qname(a))?;
    let p: &'static [u8] = core::mem::transmute::<&[u8], &'static [u8]>(p);
    Some((p, field(doc, doc.node(a).value)))
}

struct C14nNs {
    prefix: &'static [u8],
    uri: &'static [u8],
}

unsafe fn c14n_nearest(doc: &XmlDoc, node: NodeId, prefix: &[u8]) -> Option<&'static [u8]> {
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

unsafe fn c14n_namespaces(doc: &XmlDoc, n: NodeId, is_apex: bool) -> Result<Vec<C14nNs>, ()> {
    let mut out: Vec<C14nNs> = Vec::new();
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
                            let above = c14n_nearest(doc, id, p);
                            if above == Some(u) {
                                false
                            } else {
                                !(p.is_empty() && u.is_empty())
                                    || above.is_some_and(|a| !a.is_empty())
                            }
                        };
                        if keep {
                            out.mkr_reserve(1)?;
                            out.push(C14nNs { prefix: p, uri: u });
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
    out.sort_by(|a, b| a.prefix.cmp(b.prefix));
    Ok(out)
}

unsafe fn c14n_node(
    b: *mut Buf,
    doc: &XmlDoc,
    n: NodeId,
    is_apex: bool,
    comments: bool,
    depth: u32,
) -> W {
    match doc.type_(n) {
        Some(NodeType::Element) => {
            if depth as usize >= MAX_DEPTH {
                return Err(());
            }
            put(b, b"<")?;
            put(b, field(doc, doc.node(n).qname))?;

            for ns in c14n_namespaces(doc, n, is_apex)? {
                if ns.prefix.is_empty() {
                    put(b, b" xmlns=\"")?;
                } else {
                    put(b, b" xmlns:")?;
                    put(b, ns.prefix)?;
                    put(b, b"=\"")?;
                }
                c14n_escaped(b, ns.uri, true)?;
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
            attrs.sort_by(|&x, &y| {
                field(doc, doc.node(x).ns_uri)
                    .cmp(field(doc, doc.node(y).ns_uri))
                    .then_with(|| field(doc, doc.node(x).local).cmp(field(doc, doc.node(y).local)))
            });
            for at in attrs {
                put(b, b" ")?;
                put(b, field(doc, doc.node(at).qname))?;
                put(b, b"=\"")?;
                c14n_escaped(b, field(doc, doc.node(at).value), true)?;
                put(b, b"\"")?;
            }

            put(b, b">")?;
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                c14n_node(b, doc, cid, false, comments, depth + 1)?;
                c = doc.next(cid);
            }
            put(b, b"</")?;
            put(b, field(doc, doc.node(n).qname))?;
            put(b, b">")
        }
        Some(NodeType::Text | NodeType::CData) => {
            c14n_escaped(b, field(doc, doc.node(n).value), false)
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
                c14n_node(b, doc, cid, false, comments, depth)?;
                c = doc.next(cid);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
