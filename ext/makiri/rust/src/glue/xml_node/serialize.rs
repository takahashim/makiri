//! Turning an XML node back into text (glue/ruby_xml_node_serialize.c).
//!
//!   `#to_xml` / `#to_s`   XML 1.0, optionally indented, optionally transcoded
//!   `#canonicalize`       Inclusive Canonical XML 1.0
//!   `#to_html` and friends are refused rather than answered wrongly.
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
//! borrow to `'static`: serialization holds the Document wrapper for the whole
//! call (nothing can drop or mutate it under the GVL), which is the same claim
//! the C made for the arena's lifetime.

#![allow(clippy::missing_safety_doc)]

use crate::falloc::Reserve;
use core::ffi::{c_int, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};
use rb_sys::VALUE;

use super::abi::*;
use super::{node_document, unwrap};
use crate::cbuf::{mkr_buf_append, Buf, MKR_OK};
use crate::glue::abi::is_kind_of;

/* Taken from `crate::xml::abi`, the XML engine's own declaration. */
use crate::xml::abi::{FLAG_DOM_LOOSE_NAME, MAX_DEPTH};

/// The XML document behind `rb_self`'s wrapper.
unsafe fn xdoc(rb_self: Value) -> *mut XmlDoc {
    crate::glue::xml_node::mkr_doc_of(node_document(rb_self).as_raw())
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
/* #to_xml                                                            */
/* ------------------------------------------------------------------ */

unsafe fn serialize_cap(rb_self: Value) -> usize {
    let xdoc = xdoc(rb_self);
    let arena = if xdoc.is_null() {
        0
    } else {
        (*xdoc).arena_bytes
    };
    65536usize.saturating_add(arena.saturating_mul(32))
}

fn to_xml_opts(ruby: &Ruby, args: &[Value]) -> Result<(i32, Value), Error> {
    if args.is_empty() {
        return Ok((0, ruby.qnil().as_value()));
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    let h = scanned.keywords;
    let mut width = 0i32;
    if h.get(ruby.to_symbol("pretty"))
        .is_some_and(|v: Value| v.to_bool())
    {
        width = 2;
    }
    if let Some(iv) = h
        .get(ruby.to_symbol("indent"))
        .filter(|v: &Value| !v.is_nil())
    {
        let n = i32::try_convert(iv)?;
        width = n.max(0);
    }
    let enc = h
        .get(ruby.to_symbol("encoding"))
        .unwrap_or(ruby.qnil().as_value());
    Ok((width, enc))
}

fn to_xml(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let (width, enc_opt) = to_xml_opts(ruby, args)?;
    unsafe {
        let (to_enc, enc_name) = if enc_opt.is_nil() {
            (core::ptr::null_mut(), ruby.qnil().as_value())
        } else {
            let e = rb_sys::rb_to_encoding(enc_opt.as_raw());
            let name: Value = enc_opt.funcall("to_s", ())?;
            (e, name)
        };

        let doc = &*xdoc(rb_self);
        let n = unwrap(rb_self);
        if has_dom_loose_name(doc, n) {
            return Err(Error::new(
                error_class(),
                "cannot serialize XML containing a DOM-loose element name",
            ));
        }

        let mut buf = Buf::new(serialize_cap(rb_self));
        let b = &mut buf as *mut Buf;
        let rc = (|| -> W {
            if !is_kind_of(rb_self, mkr_cXmlDocument) {
                return write_node(b, doc, n, 0, width, None, 0);
            }
            let emit_enc = !enc_name.is_nil() || doc.has_encoding_decl;
            if emit_enc {
                let name = if enc_name.is_nil() {
                    ruby.str_new("UTF-8")
                } else {
                    RString::from_value(enc_name).expect("to_s answers a String")
                };
                put(b, b"<?xml version=\"1.0\" encoding=\"")?;
                put(b, name.as_slice())?;
                put(b, b"\"?>\n")?;
                core::hint::black_box(name);
            } else {
                put(b, b"<?xml version=\"1.0\"?>\n")?;
            }
            let mut c = doc.first_child(n);
            while let Some(cid) = c {
                write_node(b, doc, cid, 0, width, None, 0)?;
                put(b, b"\n")?;
                c = doc.next(cid);
            }
            Ok(())
        })();

        if rc.is_err() {
            buf.free();
            return Err(Error::new(
                error_class(),
                "failed to serialize XML: output exceeded the size limit or out of memory",
            ));
        }
        let mut str = utf8(ruby, buf.as_slice()).as_value();
        buf.free();

        if !to_enc.is_null()
            && to_enc != rb_sys::rb_utf8_encoding()
            && to_enc != rb_sys::rb_usascii_encoding()
        {
            const UNDEF_HEX_CHARREF: c_int =
                rb_sys::ruby_econv_flag_type::RUBY_ECONV_UNDEF_HEX_CHARREF as c_int;
            str = Value::from_raw(rb_sys::rb_str_encode(
                str.as_raw(),
                rb_sys::rb_enc_from_encoding(to_enc),
                UNDEF_HEX_CHARREF,
                rb_sys::Qnil as VALUE,
            ));
        }
        Ok(str)
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

fn canonicalize(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let comments = if args.is_empty() {
        false
    } else {
        let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
        scanned
            .keywords
            .get(ruby.to_symbol("comments"))
            .is_some_and(|v: Value| v.to_bool())
    };

    unsafe {
        let doc = &*xdoc(rb_self);
        let n = unwrap(rb_self);
        if has_dom_loose_name(doc, n) {
            return Err(Error::new(
                error_class(),
                "cannot canonicalize XML containing a DOM-loose element name",
            ));
        }
        let mut buf = Buf::new(serialize_cap(rb_self));
        let b = &mut buf as *mut Buf;
        let rc = (|| -> W {
            if !is_kind_of(rb_self, mkr_cXmlDocument) {
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

        if rc.is_err() {
            buf.free();
            return Err(Error::new(
                error_class(),
                "failed to canonicalize XML: output exceeded the size limit or out of memory",
            ));
        }
        let str = utf8(ruby, buf.as_slice()).as_value();
        buf.free();
        Ok(str)
    }
}

fn no_serialize(ruby: &Ruby, _rb_self: Value, _args: &[Value]) -> Result<Value, Error> {
    Err(Error::new(
        ruby.exception_not_imp_error(),
        "Makiri::XML does not HTML-serialize (to_html / inner_html / outer_html); \
         use #to_xml for XML output.",
    ))
}

/// # Safety
/// From `Init_makiri`.
pub unsafe extern "C" fn mkr_init_xml_node_serialize() {
    let m = magnus::RModule::from_value(Value::from_raw(mkr_mXmlNodeMethods))
        .expect("Makiri::XML::NodeMethods");
    for name in ["to_xml", "to_s"] {
        m.define_method(name, method!(to_xml, -1)).expect("#to_xml");
    }
    m.define_method("canonicalize", method!(canonicalize, -1))
        .expect("#canonicalize");

    for name in ["to_html", "inner_html", "outer_html"] {
        m.define_method(name, method!(no_serialize, -1))
            .expect("#to_html");
    }
}
