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
//! reads. The C kept that as a pointer into the link's own buffer, and had to
//! argue at each site about which buffer outlived which use.
//!
//! Here a prefix is a [`Prefix`], which owns its bytes inline when they were
//! invented and borrows the arena when they were not. That removes the question
//! rather than answering it: a link's prefix is valid exactly as long as the
//! link, which is what `&self` already says, and no site has to reason about
//! storage lifetimes at all.
//!
//! What remains unsafe is reading the C tree - the node pointers and their
//! arena slices - which no rearrangement here can change.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};
use crate::falloc::Reserve;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, Error, RHash, RString, Ruby, Value};
use rb_sys::VALUE;

use super::abi::*;
use super::{node_document, unwrap};
use crate::cbuf::{mkr_buf_append, Buf, MKR_OK};
use crate::glue::abi::{is_kind_of, mkr_doc_parsed, mkr_parsed_xml_doc};

/* Taken from `crate::xml::abi`, the XML engine's own declaration, rather than
 * restated - the same reason the status enum is imported there. */
use crate::xml::abi::{FLAG_DOM_LOOSE_NAME, MAX_DEPTH};

extern "C" {
    /// The byte-level xmlns detector on a raw name, shared with the parser.
    fn mkr_xml_xmlns_prefix(
        name: *const c_char,
        len: u32,
        prefix: *mut *const c_char,
        plen: *mut u32,
    ) -> c_int;
    fn mkr_xml_preorder_next(root: *const Node, cur: *mut Node) -> *mut Node;
}

/* ------------------------------------------------------------------ */
/* the output buffer                                                  */
/* ------------------------------------------------------------------ */

/// A write that fails closed: the buffer is capped, and every append is checked.
/// `Err(())` unwinds to the one place that turns it into a Ruby error.
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

/// A node field as a slice; a NULL pointer is empty.
unsafe fn field(ptr: *const c_char, len: u32) -> &'static [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(ptr as *const u8, len as usize)
    }
}

/* ------------------------------------------------------------------ */
/* XML escaping                                                       */
/* ------------------------------------------------------------------ */

/// Escape `&`, `<`, `>`, and - in an attribute value - `"` plus the TAB and LF
/// that attribute-value normalization would otherwise fold. CR is always
/// escaped so line-ending normalization cannot alter it on reparse.
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

/* A node's ns_uri is its IDENTITY, not something derived from the declarations
 * around it: a parsed tree carries the URIs its declarations gave it, and a node
 * that has since been moved keeps the URI it already had. So the serializer, not
 * the tree, decides which xmlns declarations the output needs - what browsers
 * do, and the reason to_xml still re-parses to the same tree after arbitrary
 * mutation.
 *
 * The in-scope bindings are a chain threaded down the recursion, ONE LINK PER
 * ELEMENT - not a per-attribute array, because an element may carry many
 * attributes and nest deeply. A link holds the element (its own xmlns attributes
 * are read straight off it) plus the single declaration the serializer may have
 * had to synthesize for the element's own name. */

/// An invented prefix is `ns` plus up to five digits, so eight bytes hold any of
/// them with room to spare.
const PREFIX_CAP: usize = 8;

/// A prefix the output will use.
///
/// It owns its bytes when they were invented, so a value of this type is valid
/// wherever the value itself is - no buffer to keep alive, and nothing for a
/// caller to copy "somewhere that lives long enough".
#[derive(Clone)]
enum Prefix {
    /// The node's own, an arena slice stable for the document's life.
    Own(&'static [u8]),
    /// Invented by [`gen_prefix`]; `ns` plus at most five digits.
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
    /// Its xmlns attributes bind at this level.
    el: *const Node,
    /// The declaration synthesized for this element's own name, if any: the
    /// prefix (owned) and the URI it binds (an arena slice).
    syn: Option<(Prefix, &'static [u8])>,
}

/// The declaration for `prefix` on `el` itself, or None.
unsafe fn own_decl(el: *const Node, prefix: &[u8]) -> Option<*const Node> {
    let mut a = (*el).attrs;
    while !a.is_null() {
        let mut p: *const c_char = core::ptr::null();
        let mut pl: u32 = 0;
        if mkr_xml_xmlns_prefix((*a).qname, (*a).qname_len, &mut p, &mut pl) != 0
            && field(p, pl) == prefix
        {
            return Some(a);
        }
        a = (*a).next;
    }
    None
}

/// What `prefix` means at this point in the walk: the URI, or None when it is
/// bound to nothing. The one traversal of the chain; the three questions the
/// serializer actually asks are the thin wrappers below.
unsafe fn lookup<'a>(scope: Option<&'a Scope<'a>>, prefix: &[u8]) -> Option<&'a [u8]> {
    let mut s = scope;
    while let Some(cur) = s {
        if let Some(d) = own_decl(cur.el, prefix) {
            return Some(field((*d).value, (*d).value_len));
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

/// `xml` is bound by the spec everywhere and may not be redeclared, so it is
/// never invented and never declared.
fn is_xml_prefix(prefix: &[u8]) -> bool {
    prefix == b"xml"
}

/// True when `prefix` already means exactly `uri` here. An undeclared DEFAULT
/// means no namespace, so an unprefixed name in no namespace needs no
/// declaration.
unsafe fn bound_to(scope: Option<&Scope>, prefix: &[u8], uri: &[u8]) -> bool {
    if is_xml_prefix(prefix) {
        return true;
    }
    match lookup(scope, prefix) {
        None => prefix.is_empty() && uri.is_empty(),
        Some(got) => got == uri,
    }
}

/// True when `prefix` stands for anything at all here - so writing it would
/// SHADOW that meaning for the whole subtree.
unsafe fn is_bound(scope: Option<&Scope>, prefix: &[u8]) -> bool {
    is_xml_prefix(prefix) || lookup(scope, prefix).is_some()
}

/// Emit `xmlns="uri"` or `xmlns:prefix="uri"`.
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

/// Invents the prefixes one element needs. `seq` never rewinds, so two names on
/// the same element can never be handed the same prefix.
struct Gen {
    seq: u32,
}

/// `ns` plus the smallest free number, owned.
///
/// This is the one case where the output cannot reuse a name as written: the
/// name's own prefix already means something else here. Browsers invent too
/// (`ns1`, `ns2`, ...); without it the name would serialize under a prefix bound
/// to the wrong URI and stop round-tripping.
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
///
/// Reaching this means the prefix is not already bound to what `stop` needs, so
/// that earlier attribute is the one that declared it - and its URI says whether
/// this one can ride along on that declaration.
unsafe fn prefix_seen(el: *const Node, stop: *const Node, prefix: &[u8]) -> Option<*const Node> {
    let mut a = (*el).attrs;
    while !a.is_null() && !core::ptr::eq(a as *const Node, stop) {
        if (*a).prefix_len != 0
            && mkr_xml_xmlns_prefix(
                (*a).qname,
                (*a).qname_len,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            ) == 0
            && field((*a).prefix, (*a).prefix_len) == prefix
        {
            return Some(a);
        }
        a = (*a).next;
    }
    None
}

/// How a name is going to be written.
struct Plan {
    /// The prefix the output uses - the node's own unless one had to be
    /// invented, in which case the plan owns the bytes.
    prefix: Prefix,
    /// A declaration for it must be emitted.
    declare: bool,
}

impl Plan {
    fn bytes(&self) -> &[u8] {
        self.prefix.bytes()
    }
    /// The prefix was invented, so the local name must be re-joined rather than
    /// the qualified name written verbatim.
    fn renamed(&self) -> bool {
        self.prefix.is_invented()
    }
}

/// Plan the ELEMENT's own name.
///
/// An element MAY shadow: a declaration it writes applies to itself and its
/// subtree, and its own name is what it is for. So a prefix an ANCESTOR binds
/// differently is still written as the author had it, with a declaration here to
/// override. The one case that cannot work is the element declaring the prefix
/// itself as something else - a second, contradictory xmlns would be a duplicate
/// attribute - and a prefix is invented instead.
///
/// See [`plan_attr`] for why an ATTRIBUTE may not do the same thing.
unsafe fn plan_element(here: &Scope, n: *const Node, gen: &mut Gen) -> Option<Plan> {
    let own_prefix = field((*n).prefix, (*n).prefix_len);
    let mut plan = Plan {
        prefix: Prefix::Own(own_prefix),
        declare: (*n).flags & FLAG_DOM_LOOSE_NAME == 0
            && !bound_to(Some(here), own_prefix, field((*n).ns_uri, (*n).ns_uri_len)),
    };
    if plan.declare && own_decl(n, own_prefix).is_some() {
        plan.prefix = gen_prefix(Some(here), gen)?;
    }
    Some(plan)
}

/// Plan a prefixed ATTRIBUTE's name.
///
/// Unlike an element, an attribute may NOT shadow: the declaration it would need
/// sits on the element and would rebind the prefix for every descendant, quietly
/// changing what they stand for. So only a prefix bound NOWHERE is written as
/// the author had it; anything already spoken for takes an invented prefix,
/// which is free by construction and shadows nothing. (Browsers do the same.)
/// This is the one place the two planners differ - change one rule and look at
/// the other.
///
///   already means this URI here         -> as-is, no declaration
///   an earlier attribute declared it so -> as-is, no declaration
///   bound to something else, or an
///     earlier attribute claimed it      -> invent a prefix, declare that
///   bound to nothing                    -> as-is, declare it
unsafe fn plan_attr(
    here: &Scope,
    el: *const Node,
    a: *const Node,
    gen: &mut Gen,
) -> Option<Plan> {
    let own_prefix = field((*a).prefix, (*a).prefix_len);
    let mut plan = Plan { prefix: Prefix::Own(own_prefix), declare: false };

    /* An unprefixed attribute is in no namespace - the default never applies to
     * one - and a declaration declares itself. */
    let is_decl = mkr_xml_xmlns_prefix(
        (*a).qname,
        (*a).qname_len,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    ) != 0;
    if own_prefix.is_empty() || is_decl {
        return Some(plan);
    }
    let uri = field((*a).ns_uri, (*a).ns_uri_len);
    if bound_to(Some(here), own_prefix, uri) {
        return Some(plan);
    }

    let prior = prefix_seen(el, a, own_prefix);
    let taken = is_bound(Some(here), own_prefix);
    if !taken {
        if let Some(p) = prior {
            if field((*p).ns_uri, (*p).ns_uri_len) == uri {
                return Some(plan); /* that earlier attribute already declared exactly this */
            }
        }
    }
    if taken || prior.is_some() {
        plan.prefix = gen_prefix(Some(here), gen)?;
    }
    plan.declare = true;
    Some(plan)
}

/// Write `n`'s name: its qualified name verbatim, or - when a prefix had to be
/// invented - that prefix with its local name.
unsafe fn write_name(b: *mut Buf, n: *const Node, plan: &Plan) -> W {
    if !plan.renamed() {
        return put(b, field((*n).qname, (*n).qname_len));
    }
    put(b, plan.bytes())?;
    put(b, b":")?;
    put(b, field((*n).local, (*n).local_len))
}

unsafe fn indent(b: *mut Buf, level: i32, width: i32) -> W {
    put(b, b"\n")?;
    for _ in 0..level * width {
        put(b, b" ")?;
    }
    Ok(())
}

/// True if any child is character data: such an element stays inline even in
/// pretty mode, so its text content is preserved exactly.
unsafe fn has_chardata(e: *const Node) -> bool {
    let mut c = (*e).first_child;
    while !c.is_null() {
        if matches!((*c).type_, T_TEXT | T_CDATA) {
            return true;
        }
        c = (*c).next;
    }
    false
}

unsafe fn has_dom_loose_name(root: *const Node) -> bool {
    let mut cur = root as *mut Node;
    while !cur.is_null() {
        if (*cur).type_ == T_ELEMENT && (*cur).flags & FLAG_DOM_LOOSE_NAME != 0 {
            return true;
        }
        cur = mkr_xml_preorder_next(root, cur);
    }
    false
}

/// `<!DOCTYPE name [PUBLIC "pub" "sys" | SYSTEM "sys"]>` from the off-tree node.
unsafe fn write_doctype(b: *mut Buf, dt: *const Node) -> W {
    put(b, b"<!DOCTYPE ")?;
    put(b, field((*dt).local, (*dt).local_len))?;
    if !(*dt).prefix.is_null() {
        /* a PUBLIC id is present -> PUBLIC pub sys */
        put(b, b" PUBLIC \"")?;
        put(b, field((*dt).prefix, (*dt).prefix_len))?;
        put(b, b"\" \"")?;
        put(b, field((*dt).value, (*dt).value_len))?;
        put(b, b"\"")?;
    } else if !(*dt).value.is_null() {
        put(b, b" SYSTEM \"")?;
        put(b, field((*dt).value, (*dt).value_len))?;
        put(b, b"\"")?;
    }
    put(b, b">")
}

/// `depth` is the ELEMENT nesting this walk is already inside, capped at the
/// reader's own limit - so exactly the documents it accepts are the ones that
/// serialize.
///
/// Counting anything else would refuse documents the reader takes: a character,
/// comment or PI node at the deepest element is not another level, and neither
/// is a fragment (it has no markup of its own). Only the element case tests and
/// advances it.
///
/// Two reasons, one of which is this function's own contract. A tree built with
/// the factories has no depth limit - only parsing does - so a deeper-than-cap
/// document would serialize to XML that Makiri itself cannot read back, breaking
/// "output re-parses to the same tree". And the walk recurses, so a deep enough
/// tree would exhaust the stack before it got there; the cap turns a crash into
/// a clean error.
unsafe fn write_node(
    b: *mut Buf,
    n: *const Node,
    level: i32,
    width: i32,
    scope: Option<&Scope>,
    depth: u32,
) -> W {
    match (*n).type_ {
        T_DOCTYPE => write_doctype(b, n),
        T_ELEMENT => {
            if depth as usize >= MAX_DEPTH {
                return Err(());
            }
            /* This element's link: its own xmlns attributes bind here, plus at
             * most one declaration synthesized for its own name. The link owns
             * the storage for an invented prefix, so nothing the element does
             * afterwards can move it out from under a descendant. */
            let mut here = Scope { up: scope, el: n, syn: None };
            let mut gen = Gen { seq: 1 }; /* shared by the names on this element */

            /* Decide the name before writing anything: the name comes first in
             * the output, but an invented prefix is only known once a
             * declaration is. */
            let el = plan_element(&here, n, &mut gen).ok_or(())?;

            put(b, b"<")?;
            write_name(b, n, &el)?;
            if el.declare {
                declare(b, el.bytes(), field((*n).ns_uri, (*n).ns_uri_len))?;
                /* The link takes its own copy of the prefix, so it is valid for
                 * exactly as long as the link - which is the whole subtree. */
                here.syn = Some((el.prefix.clone(), field((*n).ns_uri, (*n).ns_uri_len)));
            }

            /* Each attribute, preceded by the declaration it needs - the order a
             * browser emits. */
            let mut a = (*n).attrs;
            while !a.is_null() {
                let at = plan_attr(&here, n, a, &mut gen).ok_or(())?;
                if at.declare {
                    declare(b, at.bytes(), field((*a).ns_uri, (*a).ns_uri_len))?;
                }
                put(b, b" ")?;
                write_name(b, a, &at)?;
                put(b, b"=\"")?;
                escaped(b, field((*a).value, (*a).value_len), true)?;
                put(b, b"\"")?;
                a = (*a).next;
            }

            if (*n).first_child.is_null() {
                return put(b, b"/>");
            }
            put(b, b">")?;
            let block = width > 0 && !has_chardata(n);
            let mut c = (*n).first_child;
            while !c.is_null() {
                if block {
                    indent(b, level + 1, width)?;
                }
                write_node(b, c, level + 1, width, Some(&here), depth + 1)?;
                c = (*c).next;
            }
            if block {
                indent(b, level, width)?;
            }
            put(b, b"</")?;
            write_name(b, n, &el)?;
            put(b, b">")
        }
        T_TEXT => escaped(b, field((*n).value, (*n).value_len), false),
        T_CDATA => {
            put(b, b"<![CDATA[")?;
            put(b, field((*n).value, (*n).value_len))?;
            put(b, b"]]>")
        }
        T_COMMENT => {
            put(b, b"<!--")?;
            put(b, field((*n).value, (*n).value_len))?;
            put(b, b"-->")
        }
        T_PI => {
            put(b, b"<?")?;
            put(b, field((*n).local, (*n).local_len))?;
            if (*n).value_len != 0 {
                put(b, b" ")?;
                put(b, field((*n).value, (*n).value_len))?;
            }
            put(b, b"?>")
        }
        T_FRAGMENT => {
            /* A fragment has no markup of its own: it serializes as its
             * children, in order, spliced together. */
            let mut c = (*n).first_child;
            while !c.is_null() {
                write_node(b, c, level, width, scope, depth)?;
                c = (*c).next;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/* ------------------------------------------------------------------ */
/* #to_xml                                                            */
/* ------------------------------------------------------------------ */

/// An upper bound for a serialization buffer, scaled to the document's tracked
/// arena bytes.
///
/// The serialized form of any acyclic, depth-bounded document is a small
/// multiple of its arena bytes, so 32x - covering worst-case escaping and
/// maximal pretty-print indentation - admits every legitimate serialization,
/// with a 64 KiB floor for tiny documents. A cyclic or pathologically deep
/// CONSTRUCTED tree exceeds the bound and fails closed instead of growing the
/// buffer without limit. Defence in depth: the mutation guards already prevent
/// cycles.
unsafe fn serialize_cap(rb_self: Value) -> usize {
    let xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(node_document(rb_self).as_raw())) as *mut XmlDoc;
    let arena = if xdoc.is_null() { 0 } else { (*xdoc).arena_bytes };
    65536usize.saturating_add(arena.saturating_mul(32))
}

/// Read the `pretty:` / `indent:` / `encoding:` keywords.
fn to_xml_opts(ruby: &Ruby, args: &[Value]) -> Result<(i32, Value), Error> {
    if args.is_empty() {
        return Ok((0, ruby.qnil().as_value()));
    }
    let scanned = magnus::scan_args::scan_args::<(), (), (), (), RHash, ()>(args)?;
    let h = scanned.keywords;
    let mut width = 0i32;
    if h.get(ruby.to_symbol("pretty")).is_some_and(|v: Value| v.to_bool()) {
        width = 2;
    }
    if let Some(iv) = h.get(ruby.to_symbol("indent")).filter(|v: &Value| !v.is_nil()) {
        let n = i32::try_convert(iv)?;
        width = n.max(0);
    }
    let enc = h
        .get(ruby.to_symbol("encoding"))
        .unwrap_or(ruby.qnil().as_value());
    Ok((width, enc))
}

/// `node.to_xml(pretty: false, indent: 2, encoding: "UTF-8")`.
///
/// A Document also emits the XML declaration and its DOCTYPE; any other node
/// serializes just its own subtree. `encoding` transcodes the output - a
/// character the target cannot represent becomes a hexadecimal character
/// reference - and is named in a Document's declaration.
fn to_xml(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let (width, enc_opt) = to_xml_opts(ruby, args)?;
    unsafe {
        /* Resolve the target encoding first: it raises on an unknown name, and
         * doing it here keeps that raise away from the live buffer below. */
        let (to_enc, enc_name) = if enc_opt.is_nil() {
            (core::ptr::null_mut(), ruby.qnil().as_value())
        } else {
            let e = rb_sys::rb_to_encoding(enc_opt.as_raw());
            let name: Value = enc_opt.funcall("to_s", ())?;
            (e, name)
        };

        let n = unwrap(rb_self);
        if has_dom_loose_name(n) {
            return Err(Error::new(
                error_class(),
                "cannot serialize XML containing a DOM-loose element name",
            ));
        }

        let mut buf = Buf::new(serialize_cap(rb_self));
        let b = &mut buf as *mut Buf;
        let rc = (|| -> W {
            if !is_kind_of(rb_self, mkr_cXmlDocument) {
                return write_node(b, n, 0, width, None, 0);
            }
            let xdoc = mkr_parsed_xml_doc(mkr_doc_parsed(rb_self.as_raw())) as *mut XmlDoc;
            /* The encoding pseudo-attribute is emitted only when an explicit
             * encoding: was asked for or the parsed source declared one, so a
             * built or declaration-less document round-trips to a bare
             * `<?xml version="1.0"?>` as Nokogiri does. The output is UTF-8
             * either way. */
            let emit_enc =
                !enc_name.is_nil() || (!xdoc.is_null() && (*xdoc).has_encoding_decl != 0);
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
            /* The DOCTYPE is a document-node child linked before the root, so
             * the child walk serializes it in place - no separate emit. */
            let mut c = (*n).first_child;
            while !c.is_null() {
                write_node(b, c, 0, width, None, 0)?;
                put(b, b"\n")?;
                c = (*c).next;
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

        /* Transcode to the requested encoding; an unrepresentable character
         * becomes a &#xNN; reference rather than raising or being dropped. */
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

/* Inclusive Canonical XML 1.0 (https://www.w3.org/TR/xml-c14n), the form used
 * for XML signatures: UTF-8 output, explicit start and end tags (no `<a/>`),
 * attributes sorted by (namespace-uri, local-name), namespace declarations
 * sorted by prefix with superfluous ones removed, CDATA emitted as escaped text,
 * comments omitted unless requested. Exclusive C14N is not implemented. */

/// C14N escaping - text: `&` `<` `>` #xD; attribute value: `&` `<` `"` #x9 #xA
/// #xD.
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

/// Read an attribute's xmlns declaration, if it is one.
unsafe fn xmlns_decl(a: *const Node) -> Option<(&'static [u8], &'static [u8])> {
    let mut p: *const c_char = core::ptr::null();
    let mut u: *const c_char = core::ptr::null();
    let (mut pl, mut ul) = (0u32, 0u32);
    if mkr_xml_node_xmlns_decl(a, &mut p, &mut pl, &mut u, &mut ul) == 0 {
        return None;
    }
    Some((field(p, pl), field(u, ul)))
}

/// A renderable namespace binding. Every slice is an arena slice, stable for the
/// document's lifetime, so the C14N walk never builds a Ruby object.
struct C14nNs {
    prefix: &'static [u8],
    uri: &'static [u8],
}

/// The nearest in-scope binding for `prefix` at or above `node`. Walks the real
/// tree; no scope dictionary is threaded.
unsafe fn c14n_nearest(node: *const Node, prefix: &[u8]) -> Option<&'static [u8]> {
    let mut e = node;
    while !e.is_null() {
        if (*e).type_ == T_ELEMENT {
            let mut a = (*e).attrs;
            while !a.is_null() {
                if let Some((p, u)) = xmlns_decl(a) {
                    if p == prefix {
                        return Some(u);
                    }
                }
                a = (*a).next;
            }
        }
        e = (*e).parent;
    }
    None
}

/// The namespace declarations to render at `n`.
///
/// The apex renders every in-scope namespace, walking ancestors with the nearest
/// binding winning; a descendant renders only its OWN declarations that change
/// the inherited binding. Sorted by prefix, the default first.
unsafe fn c14n_namespaces(n: *const Node, is_apex: bool) -> Result<Vec<C14nNs>, ()> {
    let mut out: Vec<C14nNs> = Vec::new();
    let mut default_seen = false;
    let mut e = n;
    while !e.is_null() {
        if (*e).type_ == T_ELEMENT {
            let mut a = (*e).attrs;
            while !a.is_null() {
                if let Some((p, u)) = xmlns_decl(a) {
                    if p != b"xml" {
                        /* implicit xml: is never rendered */
                        let keep = if is_apex {
                            if p.is_empty() {
                                /* the default: the nearest declaration wins, and
                                 * a nearest xmlns="" is not rendered */
                                let first = !default_seen;
                                default_seen = true;
                                first && !u.is_empty()
                            } else {
                                !out.iter().any(|x| x.prefix == p)
                            }
                        } else {
                            /* a descendant: only changes to the binding */
                            let above = c14n_nearest((*e).parent, p);
                            if above == Some(u) {
                                false /* superfluous */
                            } else {
                                /* xmlns="" only ever undeclares */
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
                a = (*a).next;
            }
        }
        if !is_apex {
            break; /* a descendant considers only its own declarations */
        }
        e = (*e).parent;
    }
    out.sort_by(|a, b| a.prefix.cmp(b.prefix));
    Ok(out)
}

unsafe fn c14n_node(b: *mut Buf, n: *const Node, is_apex: bool, comments: bool, depth: u32) -> W {
    match (*n).type_ {
        T_ELEMENT => {
            /* element nesting only, like the reader - see write_node */
            if depth as usize >= MAX_DEPTH {
                return Err(());
            }
            put(b, b"<")?;
            put(b, field((*n).qname, (*n).qname_len))?;

            for ns in c14n_namespaces(n, is_apex)? {
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

            /* attributes (non-xmlns) sorted by (namespace-uri, local-name) */
            let mut attrs: Vec<*const Node> = Vec::new();
            let mut a = (*n).attrs;
            while !a.is_null() {
                if xmlns_decl(a).is_none() {
                    attrs.mkr_reserve(1)?;
                    attrs.push(a);
                }
                a = (*a).next;
            }
            attrs.sort_by(|&x, &y| {
                field((*x).ns_uri, (*x).ns_uri_len)
                    .cmp(field((*y).ns_uri, (*y).ns_uri_len))
                    .then_with(|| field((*x).local, (*x).local_len).cmp(field((*y).local, (*y).local_len)))
            });
            for at in attrs {
                put(b, b" ")?;
                put(b, field((*at).qname, (*at).qname_len))?;
                put(b, b"=\"")?;
                c14n_escaped(b, field((*at).value, (*at).value_len), true)?;
                put(b, b"\"")?;
            }

            put(b, b">")?;
            let mut c = (*n).first_child;
            while !c.is_null() {
                c14n_node(b, c, false, comments, depth + 1)?; /* children are not the apex */
                c = (*c).next;
            }
            put(b, b"</")?;
            put(b, field((*n).qname, (*n).qname_len))?;
            put(b, b">")
        }
        /* CDATA canonicalizes to escaped text */
        T_TEXT | T_CDATA => c14n_escaped(b, field((*n).value, (*n).value_len), false),
        T_COMMENT => {
            if comments {
                put(b, b"<!--")?;
                put(b, field((*n).value, (*n).value_len))?;
                put(b, b"-->")?;
            }
            Ok(())
        }
        T_PI => {
            put(b, b"<?")?;
            put(b, field((*n).local, (*n).local_len))?;
            if (*n).value_len != 0 {
                put(b, b" ")?;
                put(b, field((*n).value, (*n).value_len))?;
            }
            put(b, b"?>")
        }
        T_FRAGMENT => {
            let mut c = (*n).first_child;
            while !c.is_null() {
                c14n_node(b, c, false, comments, depth)?; /* a fragment is not a level */
                c = (*c).next;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// `node.canonicalize(comments: false)`.
///
/// A Document canonicalizes its element plus the top-level PIs (and comments
/// when requested); any other node canonicalizes its subtree, with the apex
/// inheriting the ancestors' in-scope namespaces.
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
        let n = unwrap(rb_self);
        if has_dom_loose_name(n) {
            return Err(Error::new(
                error_class(),
                "cannot canonicalize XML containing a DOM-loose element name",
            ));
        }
        let mut buf = Buf::new(serialize_cap(rb_self));
        let b = &mut buf as *mut Buf;
        let rc = (|| -> W {
            if !is_kind_of(rb_self, mkr_cXmlDocument) {
                /* the node is the apex, inheriting the ancestors' namespaces */
                return c14n_node(b, n, true, comments, 0);
            }
            /* §2.4: a PI or comment before the document element is followed by
             * #xA, one after is preceded by #xA; the element itself has no
             * surrounding line. */
            let mut seen_root = false;
            let mut c = (*n).first_child;
            while !c.is_null() {
                if (*c).type_ == T_ELEMENT {
                    c14n_node(b, c, true, comments, 0)?; /* the root is the apex */
                    seen_root = true;
                } else if (*c).type_ == T_PI || ((*c).type_ == T_COMMENT && comments) {
                    if seen_root {
                        put(b, b"\n")?;
                    }
                    c14n_node(b, c, false, comments, 0)?;
                    if !seen_root {
                        put(b, b"\n")?;
                    }
                }
                c = (*c).next;
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

/// HTML serialization would silently misbehave on XML - the escaping, CDATA and
/// void elements all differ - so it is an explicit `NotImplementedError` rather
/// than a wrong result.
fn no_serialize(ruby: &Ruby, _rb_self: Value, _args: &[Value]) -> Result<Value, Error> {
    Err(Error::new(
        ruby.exception_not_imp_error(),
        "Makiri::XML does not HTML-serialize (to_html / inner_html / outer_html); \
         use #to_xml for XML output.",
    ))
}

/// # Safety
/// From `Init_makiri`.
#[no_mangle]
pub unsafe extern "C" fn mkr_init_xml_node_serialize() {
    let m = magnus::RModule::from_value(Value::from_raw(mkr_mXmlNodeMethods))
        .expect("Makiri::XML::NodeMethods");
    for name in ["to_xml", "to_s"] {
        m.define_method(name, method!(to_xml, -1)).expect("#to_xml");
    }
    m.define_method("canonicalize", method!(canonicalize, -1))
        .expect("#canonicalize");

    /* CSS selectors are supported on XML through the native XPath engine and are
     * registered in the XML query glue. HTML serialization is not, and says so. */
    for name in ["to_html", "inner_html", "outer_html"] {
        m.define_method(name, method!(no_serialize, -1)).expect("#to_html");
    }
}
