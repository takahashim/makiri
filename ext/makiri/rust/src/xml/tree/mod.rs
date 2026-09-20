//! Tokenizer and tree builder: the XML entry points (`parse`, `parse_ex`,
//! `parse_fragment`) and the document they build.
//!
//! The scanning is safe slice code over the input and the tree is an index
//! arena, so this module contains no `unsafe`. Reading bytes is [`cursor`]'s
//! job and checking the internal DTD subset is [`dtd`]'s; what is left here is
//! the part that actually makes nodes.

#![forbid(unsafe_code)]

mod cursor;
mod dtd;

use crate::falloc::Reserve;
use crate::xml::chars::{is_reserved_pi_target, normalize_newlines, ExpandMode};
use crate::xml::qname::{split_scanned, xmlns_prefix, Split};
use crate::xml::{
    Document, Limits, Link, NodeId, NodeType, Span, Status, MAX_ATTRS, MAX_DEPTH, MAX_NS,
    XMLNS_NS_URI, XML_NS_URI,
};
use cursor::{is_space, Cursor, ExternalId, InSlice, R};
use dtd::{Declared, Subset};

/* ---- the XML declaration's pseudo-attribute value grammars (§2.8) ----
 *
 * Naming rules they are not, so they live with the declaration parser that is
 * their only consumer rather than in `qname`. */

fn is_version_num(s: &[u8]) -> bool {
    s.len() >= 3 && s.starts_with(b"1.") && s[2..].iter().all(|b| b.is_ascii_digit())
}

fn is_enc_name(s: &[u8]) -> bool {
    match s.first() {
        Some(c0) if c0.is_ascii_alphabetic() => s[1..]
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-'),
        _ => false,
    }
}

fn is_yes_no(s: &[u8]) -> bool {
    s == b"yes" || s == b"no"
}

/// A namespace binding in scope: prefix ("" = default) -> byte-store span.
struct Binding {
    pfx: Vec<u8>,
    uri: Span,
}

/// One attribute of the current start tag before namespace resolution.
#[derive(Clone, Copy)]
struct RawAttr {
    name: InSlice,
    val: InSlice,
}

pub struct Parser<'a> {
    cur: Cursor<'a>,
    doc: &'a mut Document,
    fragment: Option<NodeId>,
    binds: Vec<Binding>,
    ratt: Vec<RawAttr>,
    stack: Vec<NodeId>,
    frame: Vec<usize>,
    saw_doctype: bool,
    /// What a DOCTYPE declared, which outlives the DOCTYPE: a reference to one
    /// of its entities is refused wherever it occurs, not where it was declared.
    declared: Declared,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8], doc: &'a mut Document, fragment: Option<NodeId>) -> Self {
        Parser {
            cur: Cursor::new(input),
            doc,
            fragment,
            binds: Vec::new(),
            ratt: Vec::new(),
            stack: Vec::new(),
            frame: Vec::new(),
            saw_doctype: false,
            declared: Declared::default(),
        }
    }

    fn status(&self) -> Status {
        self.cur.status
    }

    /* ---- arena ---- */

    /// The parent a node created at the cursor attaches to.
    #[inline]
    fn cur_parent(&self) -> NodeId {
        match self.stack.last() {
            Some(&n) => n,
            None => self.fragment.unwrap_or_else(|| self.doc.doc_node()),
        }
    }

    /// Map a `Document` allocation error onto the parse status.
    #[inline]
    fn arena<T>(&mut self, r: Result<T, Status>) -> R<T> {
        match r {
            Ok(v) => Ok(v),
            Err(st) => self.cur.fail(st),
        }
    }

    /// Copy a slice into the arena (fails closed on budget / OOM).
    fn own(&mut self, s: InSlice) -> R<Span> {
        let bytes = self.cur.slice(s);
        let r = self.doc.store(bytes);
        self.arena(r)
    }

    /// Expand references into the arena. A reference to an entity a DTD
    /// declared is not a syntax error but a construct Makiri refuses.
    fn expand(&mut self, s: &[u8], mode: ExpandMode) -> R<Span> {
        let r = self.doc.expand(s, mode);
        if r == Err(Status::Syntax) && self.declared.refs_unexpanded_entity(self.cur.input(), s) {
            return self.cur.unsupported();
        }
        self.arena(r)
    }

    fn new_node(&mut self, ty: NodeType) -> R<NodeId> {
        let r = self.doc.new_node(ty);
        self.arena(r)
    }

    /// Append a TEXT / CDATA node, coalescing with a preceding sibling of the
    /// SAME type (as libxml2 / the XPath data model do).
    fn append_chardata(&mut self, parent: NodeId, ty: NodeType, val: Span) -> R {
        let r = self.doc.append_chardata(parent, ty, val);
        self.arena(r)
    }

    /// Store `name` (prefix:local per `sp`) as one arena copy on `node`.
    fn set_node_qname(&mut self, node: NodeId, name: &[u8], sp: &Split) -> R {
        let r = self
            .doc
            .assign_qname(node, name, sp.prefix_len, sp.local_off, sp.local_len);
        self.arena(r)
    }

    /* ---- namespaces (§7) ---- */

    fn ns_lookup(&self, pfx: &[u8]) -> Option<Span> {
        if pfx == b"xml" {
            return Some(self.doc.xml_ns_span());
        }
        self.binds
            .iter()
            .rev()
            .find(|b| b.pfx == pfx)
            .map(|b| b.uri)
    }

    fn push_binding(&mut self, pfx: &[u8], uri: Span) -> R {
        if self.binds.len() + 1 > MAX_NS {
            return self.cur.limit();
        }
        let mut v: Vec<u8> = Vec::new();
        if v.falloc_reserve_exact(pfx.len()).is_err() || self.binds.falloc_reserve(1).is_err() {
            return self.cur.fail(Status::Oom);
        }
        v.extend_from_slice(pfx);
        self.binds.push(Binding { pfx: v, uri });
        Ok(())
    }

    /* ---- start tag: four ordered phases ---- */

    /// Phase 1: scan the raw attributes + the tag close. Ok(true) on '>',
    /// Ok(false) on '/>'.
    fn scan_raw_attrs(&mut self) -> R<bool> {
        self.ratt.clear();
        loop {
            self.cur.skip_ws();
            let c = match self.cur.peek() {
                Some(c) => c,
                None => return self.cur.syntax(), /* unterminated tag */
            };
            if c == b'>' {
                self.cur.advance();
                return Ok(true);
            }
            if c == b'/' {
                self.cur.advance();
                if self.cur.peek() != Some(b'>') {
                    return self.cur.syntax();
                }
                self.cur.advance();
                return Ok(false);
            }
            let name = self.cur.scan_name()?;
            self.cur.skip_ws();
            if self.cur.peek() != Some(b'=') {
                return self.cur.syntax();
            }
            self.cur.advance();
            self.cur.skip_ws();
            let q = match self.cur.peek() {
                Some(q @ (b'"' | b'\'')) => q,
                _ => return self.cur.syntax(),
            };
            self.cur.advance();
            let vs = self.cur.pos();
            loop {
                match self.cur.peek() {
                    None => break,
                    Some(vc) if vc == q => break,
                    Some(b'<') => return self.cur.syntax(), /* raw '<' in AttValue */
                    Some(_) => self.cur.advance(),
                }
            }
            if self.cur.left() == 0 {
                return self.cur.syntax(); /* unterminated value */
            }
            let val = self.cur.taken_since(vs)?;
            self.cur.advance(); /* closing quote */
            /* §3.1: attributes are S-separated */
            if let Some(nx) = self.cur.peek() {
                if nx != b'>' && nx != b'/' && !is_space(nx) {
                    return self.cur.syntax();
                }
            }
            if self.ratt.len() + 1 > MAX_ATTRS {
                return self.cur.limit();
            }
            if self.ratt.falloc_reserve(1).is_err() {
                return self.cur.fail(Status::Oom);
            }
            self.ratt.push(RawAttr { name, val });
        }
    }

    /// Phase 2: xmlns declarations into the scope bindings (§7.1 rules).
    fn apply_xmlns_bindings(&mut self) -> R {
        for i in 0..self.ratt.len() {
            let r = self.ratt[i];
            let name = self.cur.slice(r.name);
            let bpfx = match xmlns_prefix(name) {
                Some(p) => p,
                None => continue,
            };
            if bpfx == b"xmlns" {
                return self.cur.syntax(); /* xmlns:xmlns reserved */
            }
            let val = self.cur.slice(r.val);
            let uri = self.expand(val, ExpandMode::Attr)?;
            let ub = self.doc.span(uri);
            if bpfx == b"xml" {
                if ub != XML_NS_URI {
                    return self.cur.syntax();
                }
            } else if ub == XML_NS_URI || ub == XMLNS_NS_URI {
                return self.cur.syntax(); /* reserved URI bound to another prefix */
            }
            if !bpfx.is_empty() && ub.is_empty() {
                return self.cur.syntax(); /* xmlns:p="" (XML 1.0) */
            }
            self.push_binding(bpfx, uri)?;
        }
        Ok(())
    }

    /// Phase 3: the element's own namespace URI.
    fn resolve_element_ns(&mut self, el: NodeId) -> R {
        let pfx_span = self.doc.node(el).prefix;
        if pfx_span.len > 0 {
            let pfx = self.doc.span(pfx_span);
            if pfx == b"xmlns" {
                return self.cur.syntax();
            }
            match self.ns_lookup(pfx) {
                Some(span) => self.doc.node_mut(el).ns_uri = span,
                None => return self.cur.syntax(), /* unbound prefix */
            }
        } else if let Some(span) = self.ns_lookup(b"") {
            if span.len > 0 {
                self.doc.node_mut(el).ns_uri = span;
            }
        }
        /* Decided: from here the URI is the node's identity (lib.rs). A
         * parsed element in no namespace is resolved too - "no namespace" is
         * a decision, not an absence, and moving it under a default
         * namespace must not silently put it in one. */
        self.doc.node_mut(el).flags |= crate::xml::FLAG_NS_RESOLVED;
        Ok(())
    }

    /// Phase 4: the attribute nodes (xmlns kept, §7.2), then duplicates (§9.3).
    fn build_attr_nodes(&mut self, el: NodeId) -> R {
        let mut tail: Option<NodeId> = None;
        for i in 0..self.ratt.len() {
            let r = self.ratt[i];
            let name = self.cur.slice(r.name);
            let sp = match split_scanned(name) {
                Some(s) => s,
                None => return self.cur.syntax(),
            };
            let attr = self.new_node(NodeType::Attribute)?;
            self.set_node_qname(attr, name, &sp)?;
            if xmlns_prefix(name).is_some() {
                let span = self.doc.xmlns_ns_span();
                self.doc.node_mut(attr).ns_uri = span;
            } else if sp.prefix_len > 0 {
                match self.ns_lookup(&name[..sp.prefix_len as usize]) {
                    Some(span) => self.doc.node_mut(attr).ns_uri = span,
                    None => return self.cur.syntax(), /* unbound prefix */
                }
            }
            let val = self.cur.slice(r.val);
            let v = self.expand(val, ExpandMode::Attr)?;
            self.doc.node_mut(attr).value = v;
            self.doc.set_parent(attr, Some(el));
            match tail {
                None => self.doc.node_mut(el).attrs = Link::of(attr),
                Some(t) => self.doc.node_mut(t).next = Link::of(attr),
            }
            tail = Some(attr);
        }
        /* §9.3: no two attributes share (namespace URI, local name) */
        if self.doc.has_duplicate_attributes(el) {
            return self.cur.syntax();
        }
        Ok(())
    }

    fn parse_element_body(&mut self, el: NodeId) -> R<bool> {
        let pushed = self.scan_raw_attrs()?;
        self.apply_xmlns_bindings()?;
        self.resolve_element_ns(el)?;
        self.build_attr_nodes(el)?;
        Ok(pushed)
    }

    /* ---- markup ---- */

    /// '<!--' comment (cursor at '!').
    fn parse_comment(&mut self, parent: NodeId) -> R {
        self.cur.advance_n(3);
        let body = self.cur.scan_until_close(b"-->", Some(b'-'))?;
        let c = self.new_node(NodeType::Comment)?;
        let v = self.own(body)?;
        self.doc.node_mut(c).value = v;
        self.doc.append_child(parent, c);
        self.cur.take_close(body, b"-->");
        Ok(())
    }

    /// '<![CDATA[' section (cursor at '!').
    fn parse_cdata(&mut self, parent: NodeId) -> R {
        if self.stack.is_empty() && self.fragment.is_none() {
            return self.cur.syntax(); /* CDATA outside the root */
        }
        self.cur.advance_n(8);
        let body = self.cur.scan_until_close(b"]]>", None)?;
        let cval = self.own(body)?;
        self.append_chardata(parent, NodeType::CData, cval)?;
        self.cur.take_close(body, b"]]>");
        Ok(())
    }

    /* ---- XML declaration (§2.8) ---- */

    fn decl_eq(&mut self) -> R {
        self.cur.skip_ws();
        if self.cur.peek() != Some(b'=') {
            return self.cur.syntax();
        }
        self.cur.advance();
        self.cur.skip_ws();
        Ok(())
    }

    fn decl_value(&mut self, ok: fn(&[u8]) -> bool) -> R {
        let v = self.cur.parse_quoted()?;
        if !ok(self.cur.slice(v)) {
            return self.cur.syntax();
        }
        Ok(())
    }

    /// '<?xml' consumed. version (encoding)? (standalone)? S? '?>'
    fn parse_xml_decl_body(&mut self) -> R {
        self.cur.require_space()?;
        if !self.cur.eat_keyword(b"version") {
            return self.cur.syntax();
        }
        self.decl_eq()?;
        let ver = self.cur.parse_quoted()?;
        /* §2.8: any 1.x. A 1.0 processor reads a 1.x document as 1.0, so one
         * that uses a 1.1-only feature fails on that feature, not its label. */
        if !is_version_num(self.cur.slice(ver)) {
            return self.cur.syntax();
        }
        let (mut saw_enc, mut saw_sd) = (false, false);
        loop {
            let had_s = self.cur.skip_some_ws();
            if self.cur.starts(b"?>") {
                self.cur.advance_n(2);
                return Ok(());
            }
            if !had_s {
                return self.cur.syntax();
            }
            if !saw_enc && !saw_sd && self.cur.eat_keyword(b"encoding") {
                saw_enc = true;
                self.doc.mark_encoding_decl();
                self.decl_eq()?;
                self.decl_value(is_enc_name)?;
            } else if !saw_sd && self.cur.eat_keyword(b"standalone") {
                saw_sd = true;
                self.decl_eq()?;
                self.decl_value(is_yes_no)?;
            } else {
                return self.cur.syntax();
            }
        }
    }

    /// '<?' processing instruction (cursor at '?').
    fn parse_pi(&mut self, parent: NodeId, at_doc_start: bool) -> R {
        self.cur.advance_n(1);
        let t = self.cur.scan_name()?;
        let tgt = self.cur.slice(t);
        if tgt == b"xml" {
            if !at_doc_start || !self.stack.is_empty() {
                return self.cur.syntax();
            }
            return self.parse_xml_decl_body();
        }
        if is_reserved_pi_target(tgt) {
            return self.cur.syntax(); /* reserved target ("XML"/"xmL"/...) */
        }
        if tgt.contains(&b':') {
            return self.cur.syntax(); /* Namespaces in XML §7: a PI target is an NCName */
        }
        if !self.cur.starts(b"?>") {
            self.cur.require_space()?;
        }
        let body = self.cur.scan_until_close(b"?>", None)?;
        let pi = self.new_node(NodeType::Pi)?;
        let lp = self.own(t)?;
        let vp = self.own(body)?;
        {
            let n = self.doc.node_mut(pi);
            n.local = lp;
            n.value = vp;
        }
        self.doc.append_child(parent, pi);
        self.cur.take_close(body, b"?>");
        Ok(())
    }

    /// End tag '</name S? >' (cursor at '/').
    fn parse_end_tag(&mut self) -> R {
        self.cur.advance();
        let nm = self.cur.scan_name()?;
        self.cur.skip_ws();
        if self.cur.peek() != Some(b'>') {
            return self.cur.syntax();
        }
        self.cur.advance();
        let top = match self.stack.last() {
            Some(&t) => t,
            None => return self.cur.syntax(), /* end tag with no open element */
        };
        let name = self.cur.slice(nm);
        let pfx_span = self.doc.node(top).prefix;
        let matched = if pfx_span.len > 0 {
            let pfx = self.doc.span(pfx_span);
            let local = self.doc.local(top);
            let pl = pfx.len();
            name.len() == pl + 1 + local.len()
                && name.starts_with(pfx)
                && name[pl] == b':'
                && &name[pl + 1..] == local
        } else {
            name == self.doc.local(top)
        };
        if !matched {
            return self.cur.syntax(); /* mismatched end tag */
        }
        self.stack.pop();
        let base = self.frame.pop().unwrap_or(0);
        self.binds.truncate(base); /* pop this element's namespace scope */
        Ok(())
    }

    /// '<!DOCTYPE' (cursor at '!').
    ///
    /// The internal subset is checked by [`dtd`], which keeps nothing; an
    /// external subset is named but, as §5.1 allows, never read.
    fn parse_doctype(&mut self) -> R {
        if !self.stack.is_empty() || self.doc.root().is_some() || self.saw_doctype {
            return self.cur.syntax();
        }
        self.saw_doctype = true;
        self.cur.advance_n(8); /* "!DOCTYPE" */
        self.cur.require_space()?;
        let name = self.cur.scan_qname()?;

        let mut ids = ExternalId::default();
        if self.cur.peek().is_some_and(is_space) {
            self.cur.skip_ws();
            if self.cur.starts(b"SYSTEM") || self.cur.starts(b"PUBLIC") {
                ids = self.cur.scan_external_id(false)?;
                self.cur.skip_ws();
            }
        }
        self.declared.note_external_subset(ids.system.is_some());

        let unsupported = if self.cur.peek() == Some(b'[') {
            self.cur.advance();
            let u = Subset::new(&mut self.cur, &mut self.declared).parse()?;
            self.cur.advance(); /* ']' */
            self.cur.skip_ws();
            u
        } else {
            false
        };
        if self.cur.peek() != Some(b'>') {
            return self.cur.syntax(); /* unterminated DOCTYPE */
        }
        self.cur.advance();
        if unsupported {
            return self.cur.unsupported();
        }

        let dt = self.new_node(NodeType::Doctype)?;
        let nm = self.own(name)?;
        {
            let d = self.doc.node_mut(dt);
            d.local = nm;
            d.qname = nm;
        }
        if let Some(p) = ids.public {
            let p = self.own(p)?;
            self.doc.node_mut(dt).prefix = p;
        }
        if let Some(s) = ids.system {
            let s = self.own(s)?;
            self.doc.node_mut(dt).value = s;
        }
        let dn = self.doc.doc_node();
        self.doc.append_child(dn, dt);
        self.doc.set_doctype(Some(dt));
        Ok(())
    }

    /// '<!' markup: comment, CDATA, or DOCTYPE.
    fn parse_markup(&mut self) -> R {
        let parent = self.cur_parent();
        if self.cur.starts(b"!--") {
            return self.parse_comment(parent);
        }
        if self.cur.starts(b"![CDATA[") {
            return self.parse_cdata(parent);
        }
        if self.cur.starts(b"!DOCTYPE") {
            if self.fragment.is_some() {
                return self.cur.syntax(); /* a fragment has no DOCTYPE */
            }
            return self.parse_doctype();
        }
        self.cur.syntax()
    }

    /// Start tag (cursor at the QName; tl/tc = the '<' position).
    fn parse_start_tag(&mut self, tl: u32, tc: u32) -> R {
        let nm = self.cur.scan_name()?;
        let name = self.cur.slice(nm);
        let sp = match split_scanned(name) {
            Some(s) => s,
            None => return self.cur.syntax(),
        };
        let el = self.new_node(NodeType::Element)?;
        self.set_node_qname(el, name, &sp)?;
        {
            let n = self.doc.node_mut(el);
            n.line = tl;
            n.col = tc;
        }
        if self.stack.is_empty() && self.fragment.is_none() {
            if self.doc.root().is_some() {
                return self.cur.syntax(); /* multiple roots */
            }
            self.doc.set_root(Some(el));
        }
        let parent = self.cur_parent();
        self.doc.append_child(parent, el);
        let bind_base = self.binds.len();
        let pushed = self.parse_element_body(el)?;
        if pushed {
            if self.stack.len() + 1 > MAX_DEPTH {
                return self.cur.limit();
            }
            if self.stack.falloc_reserve(1).is_err() || self.frame.falloc_reserve(1).is_err() {
                return self.cur.fail(Status::Oom);
            }
            self.stack.push(el);
            self.frame.push(bind_base);
        } else {
            self.binds.truncate(bind_base); /* self-closing: pop its scope now */
        }
        Ok(())
    }

    /// Character data up to the next '<'.
    fn parse_text(&mut self) -> R {
        let tstart = self.cur.pos();
        let mut nonspace = false;
        while let Some(c) = self.cur.peek() {
            if c == b'<' {
                break;
            }
            /* §2.4: the literal "]]>" must not appear in character data */
            if c == b']' && self.cur.starts(b"]]>") {
                return self.cur.syntax();
            }
            if !is_space(c) {
                nonspace = true;
            }
            self.cur.advance();
        }
        let text = self.cur.taken_since(tstart)?;
        if self.stack.is_empty() && self.fragment.is_none() {
            if nonspace {
                return self.cur.syntax(); /* non-ws text outside any element */
            }
            return Ok(()); /* document-level whitespace: discarded */
        }
        let raw = self.cur.slice(text);
        let tv = self.expand(raw, ExpandMode::Text)?;
        let parent = self.cur_parent();
        self.append_chardata(parent, NodeType::Text, tv)
    }

    /// Tokenizer dispatch.
    fn run(&mut self) {
        while self.cur.left() > 0 {
            if self.cur.peek() != Some(b'<') {
                if self.parse_text().is_err() {
                    break;
                }
            } else {
                let (tl, tc) = (self.cur.line(), self.cur.col());
                let at_start = self.cur.at_start() && self.fragment.is_none();
                self.cur.advance(); /* '<' */
                let c = match self.cur.peek() {
                    Some(c) => c,
                    None => {
                        let _ = self.cur.syntax::<()>();
                        break;
                    }
                };
                let rc = match c {
                    b'/' => self.parse_end_tag(),
                    b'!' => self.parse_markup(),
                    b'?' => {
                        let p = self.cur_parent();
                        self.parse_pi(p, at_start)
                    }
                    _ => self.parse_start_tag(tl, tc),
                };
                if rc.is_err() {
                    break;
                }
            }
            if self.status() != Status::Ok {
                break;
            }
        }
    }

    /// Run to the end of the input, then apply the close-out rule: input can
    /// stop mid-tree, and only the caller knows whether a root was required.
    ///
    /// The whole "normalize, run, check the close-out, report" sequence used to
    /// be written out in both `parse_ex` and `parse_fragment_into`, so the rule
    /// for when a parse is finished lived twice.
    fn run_to_end(&mut self, require_root: bool) -> Result<(), Status> {
        self.run();
        let unclosed = !self.stack.is_empty() || (require_root && self.doc.root().is_none());
        if self.status() == Status::Ok && unclosed {
            let _ = self.cur.syntax::<()>();
        }
        match self.status() {
            Status::Ok => Ok(()),
            st => Err(st),
        }
    }

    /// Seed a fragment parser's scope with the document root's xmlns attributes.
    fn seed_doc_namespaces(&mut self) -> R {
        let Some(root) = self.doc.root() else {
            return Ok(());
        };
        let mut a = self.doc.attrs(root);
        while let Some(attr) = a {
            let bpfx = match xmlns_prefix(self.doc.qname(attr)) {
                Some(p) => Some(crate::falloc::try_to_vec(p).ok_or(())?),
                None => None,
            };
            if let Some(bpfx) = bpfx {
                let uri = self.doc.node(attr).value;
                self.push_binding(&bpfx, uri)?;
            }
            a = self.doc.next(attr);
        }
        Ok(())
    }
}

/// Parse `src` into a fresh document under the default budget.
pub fn parse(src: &[u8]) -> Result<Box<Document>, Status> {
    parse_ex(src, None)
}

/// Parse `src` into a fresh document. `Document::create` applies `limits` and
/// rejects an over-long source, so the budget is checked in exactly one place.
pub fn parse_ex(src: &[u8], limits: Option<&Limits>) -> Result<Box<Document>, Status> {
    let mut doc = Document::create(limits.map(|l| l.max_bytes), src.len())?;
    let norm = normalize_newlines(src)?;
    Parser::new(norm.as_deref().unwrap_or(src), &mut doc, None).run_to_end(true)?;
    Ok(doc)
}

/// Parse a fragment into a live document's arena. The document reference and
/// input slice make the ownership preconditions explicit to Rust callers.
///
/// A FAILED parse rewinds the arena. The partial fragment is unreachable - it
/// hangs off a fragment root this function never returns, and nothing already
/// in the document points into it - so its nodes and bytes are given back
/// rather than charged to the document for its lifetime. Without that, a loop
/// of rejected fragments grows a live document until every later operation
/// fails with `Limit` (`spec/xml_fragment_spec.rb` pins it).
pub fn parse_fragment(
    doc: &mut Document,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<NodeId, Status> {
    if src.len() > doc.max_bytes {
        return Err(Status::Limit);
    }
    let mark = doc.mark();
    match parse_fragment_into(doc, src, inherit_doc_ns) {
        Ok(frag) => Ok(frag),
        Err(status) => {
            doc.rewind(mark);
            Err(status)
        }
    }
}

fn parse_fragment_into(
    doc: &mut Document,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<NodeId, Status> {
    let frag = doc.new_node(NodeType::Fragment)?;
    let norm = normalize_newlines(src)?;
    let mut p = Parser::new(norm.as_deref().unwrap_or(src), doc, Some(frag));
    if inherit_doc_ns && p.seed_doc_namespaces().is_err() {
        return Err(p.status());
    }
    /* A fragment has no single-root rule. */
    p.run_to_end(false)?;
    Ok(frag)
}

#[cfg(test)]
mod tests {
    use super::{is_enc_name, is_version_num, is_yes_no};

    /// The declaration grammars, at their boundary forms. They moved here with
    /// the parser that is their only consumer (was `rust_tests.rs`).
    #[test]
    fn declaration_grammars_reject_boundary_forms() {
        assert!(is_version_num(b"1.0"));
        assert!(!is_version_num(b"1."));
        assert!(is_enc_name(b"UTF-8"));
        assert!(!is_enc_name(b"8UTF"));
        assert!(is_yes_no(b"yes"));
        assert!(is_yes_no(b"no"));
        assert!(!is_yes_no(b"Yes"));
    }
}
