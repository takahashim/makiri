//! Tokenizer + tree builder (mkr_xml_tree.c). The scanning is safe slice code
//! over the input; the tree is an index arena, so this module contains no
//! `unsafe`.

#![forbid(unsafe_code)]

use crate::falloc::Reserve;
use crate::xml::chars::{
    decode1, is_name_char, is_name_start, is_reserved_pi_target, normalize_newlines,
    validate_chars, ExpandMode,
};
use crate::xml::qname::{
    is_enc_name, is_version_num, is_yes_no, split_scanned, xmlns_prefix, Split,
};
use crate::xml::{
    Document, Link, NodeId, NodeType, Span, Status, MAX_ATTRS, MAX_DEPTH, MAX_NS, XMLNS_NS_URI,
    XML_NS_URI,
};

/// A namespace binding in scope: prefix ("" = default) -> byte-store span.
struct Binding {
    pfx: Vec<u8>,
    uri: Span,
}

/// One attribute of the current start tag before namespace resolution, as
/// (offset, len) slices of the input.
#[derive(Clone, Copy)]
struct RawAttr {
    name: (usize, usize),
    val: (usize, usize),
}

type R<T = ()> = Result<T, ()>;

#[inline]
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

#[inline]
fn find(h: &[u8], b: u8) -> Option<usize> {
    h.iter().position(|&x| x == b)
}

pub struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
    doc: &'a mut Document,
    fragment: Option<NodeId>,
    pub status: Status,
    binds: Vec<Binding>,
    ratt: Vec<RawAttr>,
    stack: Vec<NodeId>,
    frame: Vec<usize>,
    saw_doctype: bool,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8], doc: &'a mut Document, fragment: Option<NodeId>) -> Self {
        Parser {
            input,
            pos: 0,
            line: 1,
            col: 1,
            doc,
            fragment,
            status: Status::Ok,
            binds: Vec::new(),
            ratt: Vec::new(),
            stack: Vec::new(),
            frame: Vec::new(),
            saw_doctype: false,
        }
    }

    /* ---- bounded cursor ---- */

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }
    #[inline]
    fn left(&self) -> usize {
        self.input.len() - self.pos
    }
    #[inline]
    fn starts(&self, lit: &[u8]) -> bool {
        self.input[self.pos..].starts_with(lit)
    }
    #[inline]
    fn sl(&self, s: usize, n: usize) -> &'a [u8] {
        &self.input[s..s + n]
    }
    #[inline]
    fn advance(&mut self) {
        if let Some(c) = self.peek() {
            self.pos += 1;
            if c == b'\n' {
                self.line = self.line.wrapping_add(1);
                self.col = 1;
            } else {
                self.col = self.col.wrapping_add(1);
            }
        }
    }
    /// Advance up to n bytes keeping line/col correct (the span may hold LF).
    fn advance_n(&mut self, n: usize) {
        let end = core::cmp::min(self.pos.saturating_add(n), self.input.len());
        for &c in &self.input[self.pos..end] {
            if c == b'\n' {
                self.line = self.line.wrapping_add(1);
                self.col = 1;
            } else {
                self.col = self.col.wrapping_add(1);
            }
        }
        self.pos = end;
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if is_space(c) {
                self.advance();
            } else {
                break;
            }
        }
    }

    /* ---- status ---- */

    #[inline]
    fn syntax<T>(&mut self) -> R<T> {
        if self.status.is_ok() {
            self.status = Status::Syntax;
        }
        Err(())
    }
    #[inline]
    fn limit<T>(&mut self) -> R<T> {
        self.status = Status::Limit;
        Err(())
    }
    /// Map a `Document` allocation error onto the parser's status.
    #[inline]
    fn arena<T>(&mut self, r: Result<T, Status>) -> R<T> {
        match r {
            Ok(v) => Ok(v),
            Err(st) => {
                self.status = st;
                Err(())
            }
        }
    }

    #[inline]
    fn need_space(&mut self) -> R {
        match self.peek() {
            Some(c) if is_space(c) => Ok(()),
            _ => self.syntax(),
        }
    }

    /* ---- scanning ---- */

    /// Scan a Name (NameStartChar NameChar*) as an (offset, len) input slice.
    fn scan_name(&mut self) -> R<(usize, usize)> {
        let start = self.pos;
        match decode1(&self.input[self.pos..]) {
            Some((cp, bl)) if is_name_start(cp) => self.advance_n(bl),
            _ => return self.syntax(),
        }
        loop {
            match decode1(&self.input[self.pos..]) {
                Some((cp, bl)) if is_name_char(cp) => self.advance_n(bl),
                _ => break,
            }
        }
        let n = self.pos - start;
        if n > u32::MAX as usize {
            return self.limit();
        }
        Ok((start, n))
    }

    /// The parent a node created at the cursor attaches to.
    #[inline]
    fn cur_parent(&self) -> NodeId {
        match self.stack.last() {
            Some(&n) => n,
            None => self.fragment.unwrap_or_else(|| self.doc.doc_node()),
        }
    }

    /// Copy a slice into the arena (fails closed on budget / OOM).
    fn own(&mut self, s: &[u8]) -> R<Span> {
        let r = self.doc.store(s);
        self.arena(r).map_err(|_| ())
    }

    fn expand(&mut self, s: &[u8], mode: ExpandMode) -> R<Span> {
        let r = self.doc.expand(s, mode);
        self.arena(r).map_err(|_| ())
    }

    fn new_node(&mut self, ty: NodeType) -> R<NodeId> {
        let r = self.doc.new_node(ty);
        self.arena(r).map_err(|_| ())
    }

    /// Append a TEXT / CDATA node, coalescing with a preceding sibling of the
    /// SAME type (as libxml2 / the XPath data model do).
    fn append_chardata(&mut self, parent: NodeId, ty: NodeType, val: Span) -> R {
        let r = self.doc.append_chardata(parent, ty, val);
        self.arena(r).map_err(|_| ())
    }

    /// Store `name` (prefix:local per `sp`) as one arena copy on `node`.
    fn set_node_qname(&mut self, node: NodeId, name: &[u8], sp: &Split) -> R {
        let r = self
            .doc
            .assign_qname(node, name, sp.prefix_len, sp.local_off, sp.local_len);
        self.arena(r).map_err(|_| ())
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
            return self.limit();
        }
        let mut v: Vec<u8> = Vec::new();
        if v.mkr_reserve_exact(pfx.len()).is_err() || self.binds.mkr_reserve(1).is_err() {
            self.status = Status::Oom;
            return Err(());
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
            self.skip_ws();
            let c = match self.peek() {
                Some(c) => c,
                None => return self.syntax(), /* unterminated tag */
            };
            if c == b'>' {
                self.advance();
                return Ok(true);
            }
            if c == b'/' {
                self.advance();
                if self.peek() != Some(b'>') {
                    return self.syntax();
                }
                self.advance();
                return Ok(false);
            }
            let (an, alen) = self.scan_name()?;
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return self.syntax();
            }
            self.advance();
            self.skip_ws();
            let q = match self.peek() {
                Some(q @ (b'"' | b'\'')) => q,
                _ => return self.syntax(),
            };
            self.advance();
            let vs = self.pos;
            loop {
                match self.peek() {
                    None => break,
                    Some(vc) if vc == q => break,
                    Some(b'<') => return self.syntax(), /* raw '<' in AttValue */
                    Some(_) => self.advance(),
                }
            }
            if self.left() == 0 {
                return self.syntax(); /* unterminated value */
            }
            let vraw = self.pos - vs;
            if vraw > u32::MAX as usize {
                return self.limit();
            }
            self.advance(); /* closing quote */
            /* §3.1: attributes are S-separated */
            if let Some(nx) = self.peek() {
                if nx != b'>' && nx != b'/' && !is_space(nx) {
                    return self.syntax();
                }
            }
            if self.ratt.len() + 1 > MAX_ATTRS {
                return self.limit();
            }
            if self.ratt.mkr_reserve(1).is_err() {
                self.status = Status::Oom;
                return Err(());
            }
            self.ratt.push(RawAttr {
                name: (an, alen),
                val: (vs, vraw),
            });
        }
    }

    /// Phase 2: xmlns declarations into the scope bindings (§7.1 rules).
    fn apply_xmlns_bindings(&mut self) -> R {
        let input = self.input;
        for i in 0..self.ratt.len() {
            let r = self.ratt[i];
            let name = &input[r.name.0..r.name.0 + r.name.1];
            let bpfx = match xmlns_prefix(name) {
                Some(p) => p,
                None => continue,
            };
            if bpfx == b"xmlns" {
                return self.syntax(); /* xmlns:xmlns reserved */
            }
            let uri = self.expand(&input[r.val.0..r.val.0 + r.val.1], ExpandMode::Attr)?;
            let ub = self.doc.span(uri);
            if bpfx == b"xml" {
                if ub != XML_NS_URI {
                    return self.syntax();
                }
            } else if ub == XML_NS_URI || ub == XMLNS_NS_URI {
                return self.syntax(); /* reserved URI bound to another prefix */
            }
            if !bpfx.is_empty() && ub.is_empty() {
                return self.syntax(); /* xmlns:p="" (XML 1.0) */
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
                return self.syntax();
            }
            match self.ns_lookup(pfx) {
                Some(span) => self.doc.node_mut(el).ns_uri = span,
                None => return self.syntax(), /* unbound prefix */
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
        let input = self.input;
        let mut tail: Option<NodeId> = None;
        for i in 0..self.ratt.len() {
            let r = self.ratt[i];
            let name = &input[r.name.0..r.name.0 + r.name.1];
            let sp = match split_scanned(name) {
                Some(s) => s,
                None => return self.syntax(),
            };
            let attr = self.new_node(NodeType::Attribute)?;
            self.set_node_qname(attr, name, &sp)?;
            if xmlns_prefix(name).is_some() {
                let span = self.doc.xmlns_ns_span();
                self.doc.node_mut(attr).ns_uri = span;
            } else if sp.prefix_len > 0 {
                match self.ns_lookup(&name[..sp.prefix_len as usize]) {
                    Some(span) => self.doc.node_mut(attr).ns_uri = span,
                    None => return self.syntax(), /* unbound prefix */
                }
            }
            let v = self.expand(&input[r.val.0..r.val.0 + r.val.1], ExpandMode::Attr)?;
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
            return self.syntax();
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
        self.advance_n(3);
        let cstart = self.pos;
        let mut j = self.pos;
        loop {
            match find(&self.input[j..], b'-') {
                Some(at) => j += at,
                None => return self.syntax(), /* unterminated */
            }
            if self.input.get(j + 1) == Some(&b'-') {
                if self.input.get(j + 2) == Some(&b'>') {
                    break;
                }
                return self.syntax(); /* '--' not part of '-->' */
            }
            j += 1;
        }
        let craw = j - cstart;
        if craw > u32::MAX as usize {
            return self.limit();
        }
        if !validate_chars(&self.input[cstart..j]) {
            return self.syntax();
        }
        let c = self.new_node(NodeType::Comment)?;
        let v = self.own(self.sl(cstart, craw))?;
        self.doc.node_mut(c).value = v;
        self.doc.append_child(parent, c);
        self.advance_n(craw + 3);
        Ok(())
    }

    /// '<![CDATA[' section (cursor at '!').
    fn parse_cdata(&mut self, parent: NodeId) -> R {
        if self.stack.is_empty() && self.fragment.is_none() {
            return self.syntax(); /* CDATA outside the root */
        }
        self.advance_n(8);
        let cstart = self.pos;
        let mut j = self.pos;
        loop {
            match find(&self.input[j..], b']') {
                Some(at) => j += at,
                None => return self.syntax(),
            }
            if self.input.get(j + 1) == Some(&b']') && self.input.get(j + 2) == Some(&b'>') {
                break;
            }
            j += 1;
        }
        let craw = j - cstart;
        if craw > u32::MAX as usize {
            return self.limit();
        }
        if !validate_chars(&self.input[cstart..j]) {
            return self.syntax();
        }
        let cval = self.own(self.sl(cstart, craw))?;
        self.append_chardata(parent, NodeType::CData, cval)?;
        self.advance_n(craw + 3);
        Ok(())
    }

    /* ---- XML declaration (§2.8) ---- */

    fn eat_keyword(&mut self, kw: &[u8]) -> bool {
        if !self.starts(kw) {
            return false;
        }
        self.advance_n(kw.len());
        true
    }

    fn decl_eq(&mut self) -> R {
        self.skip_ws();
        if self.peek() != Some(b'=') {
            return self.syntax();
        }
        self.advance();
        self.skip_ws();
        Ok(())
    }

    /// A quoted literal as an (offset, len) input slice.
    fn parse_quoted(&mut self) -> R<(usize, usize)> {
        let q = match self.peek() {
            Some(q @ (b'"' | b'\'')) => q,
            _ => return self.syntax(),
        };
        self.advance();
        let vs = self.pos;
        while let Some(c) = self.peek() {
            if c == q {
                break;
            }
            self.advance();
        }
        if self.left() == 0 {
            return self.syntax(); /* unterminated / mismatched quote */
        }
        let n = self.pos - vs;
        if n > u32::MAX as usize {
            return self.limit();
        }
        self.advance(); /* closing quote */
        Ok((vs, n))
    }

    fn decl_value(&mut self, ok: fn(&[u8]) -> bool) -> R {
        let (vs, vl) = self.parse_quoted()?;
        if !ok(self.sl(vs, vl)) {
            return self.syntax();
        }
        Ok(())
    }

    /// '<?xml' consumed. version (encoding)? (standalone)? S? '?>'
    fn parse_xml_decl_body(&mut self) -> R {
        self.need_space()?;
        self.skip_ws();
        if !self.eat_keyword(b"version") {
            return self.syntax();
        }
        self.decl_eq()?;
        let (vs, vl) = self.parse_quoted()?;
        let ver = self.sl(vs, vl);
        if !is_version_num(ver) {
            return self.syntax();
        }
        if ver != b"1.0" {
            self.status = Status::Version; /* well-formed, unsupported version */
            return Err(());
        }
        let (mut saw_enc, mut saw_sd) = (false, false);
        loop {
            let mut had_s = false;
            while let Some(c) = self.peek() {
                if is_space(c) {
                    self.advance();
                    had_s = true;
                } else {
                    break;
                }
            }
            if self.starts(b"?>") {
                self.advance_n(2);
                return Ok(());
            }
            if !had_s {
                return self.syntax();
            }
            if !saw_enc && !saw_sd && self.eat_keyword(b"encoding") {
                saw_enc = true;
                self.doc.mark_encoding_decl();
                self.decl_eq()?;
                self.decl_value(is_enc_name)?;
            } else if !saw_sd && self.eat_keyword(b"standalone") {
                saw_sd = true;
                self.decl_eq()?;
                self.decl_value(is_yes_no)?;
            } else {
                return self.syntax();
            }
        }
    }

    /// '<?' processing instruction (cursor at '?').
    fn parse_pi(&mut self, parent: NodeId, at_doc_start: bool) -> R {
        self.advance_n(1);
        let (t, tl) = self.scan_name()?;
        let tgt = self.sl(t, tl);
        let ci_xml = is_reserved_pi_target(tgt);
        let is_decl = tgt == b"xml";
        if is_decl {
            if !at_doc_start || !self.stack.is_empty() {
                return self.syntax();
            }
            return self.parse_xml_decl_body();
        }
        if ci_xml {
            return self.syntax(); /* reserved target ("XML"/"xmL"/...) */
        }
        if !self.starts(b"?>") {
            self.need_space()?;
            self.skip_ws();
        }
        let dstart = self.pos;
        let mut j = self.pos;
        loop {
            match find(&self.input[j..], b'?') {
                Some(at) => j += at,
                None => return self.syntax(),
            }
            if self.input.get(j + 1) == Some(&b'>') {
                break;
            }
            j += 1;
        }
        let draw = j - dstart;
        if draw > u32::MAX as usize {
            return self.limit();
        }
        if !validate_chars(&self.input[dstart..j]) {
            return self.syntax();
        }
        let pi = self.new_node(NodeType::Pi)?;
        let lp = self.own(tgt)?;
        let vp = self.own(self.sl(dstart, draw))?;
        {
            let n = self.doc.node_mut(pi);
            n.local = lp;
            n.value = vp;
        }
        self.doc.append_child(parent, pi);
        self.advance_n(draw + 2);
        Ok(())
    }

    /// End tag '</name S? >' (cursor at '/').
    fn parse_end_tag(&mut self) -> R {
        self.advance();
        let (nm, nl) = self.scan_name()?;
        self.skip_ws();
        if self.peek() != Some(b'>') {
            return self.syntax();
        }
        self.advance();
        let top = match self.stack.last() {
            Some(&t) => t,
            None => return self.syntax(), /* end tag with no open element */
        };
        let name = self.sl(nm, nl);
        let pfx_span = self.doc.node(top).prefix;
        let matched = if pfx_span.len > 0 {
            let pfx = self.doc.span(pfx_span);
            let local = self.doc.local(top);
            let pl = pfx.len();
            nl == pl + 1 + local.len()
                && name.starts_with(pfx)
                && name[pl] == b':'
                && &name[pl + 1..] == local
        } else {
            name == self.doc.local(top)
        };
        if !matched {
            return self.syntax(); /* mismatched end tag */
        }
        self.stack.pop();
        let base = self.frame.pop().unwrap_or(0);
        self.binds.truncate(base); /* pop this element's namespace scope */
        Ok(())
    }

    /// '<!DOCTYPE' (cursor at '!'): recognized, not processed (§9.4).
    fn parse_doctype(&mut self) -> R {
        if !self.stack.is_empty() || self.doc.root().is_some() || self.saw_doctype {
            return self.syntax();
        }
        self.saw_doctype = true;
        self.advance_n(8); /* "!DOCTYPE" */
        self.need_space()?;
        self.skip_ws();
        let (n, nl) = self.scan_name()?;

        let mut pub_id: Option<(usize, usize)> = None;
        let mut sys_id: Option<(usize, usize)> = None;
        if let Some(c) = self.peek() {
            if is_space(c) {
                self.skip_ws();
                if self.starts(b"SYSTEM") {
                    self.advance_n(6);
                    self.need_space()?;
                    self.skip_ws();
                    sys_id = Some(self.parse_quoted()?);
                } else if self.starts(b"PUBLIC") {
                    self.advance_n(6);
                    self.need_space()?;
                    self.skip_ws();
                    pub_id = Some(self.parse_quoted()?);
                    self.need_space()?;
                    self.skip_ws();
                    sys_id = Some(self.parse_quoted()?);
                }
            }
        }

        /* skip the optional internal subset + whitespace to the true '>' */
        let mut quote: Option<u8> = None;
        let mut depth = 0usize;
        let mut closed = false;
        while let Some(c) = self.peek() {
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else if c == b'"' || c == b'\'' {
                quote = Some(c);
            } else if c == b'[' {
                depth += 1;
            } else if c == b']' {
                depth = depth.saturating_sub(1);
            } else if c == b'>' && depth == 0 {
                self.advance();
                closed = true;
                break;
            }
            self.advance();
        }
        if !closed {
            return self.syntax(); /* unterminated DOCTYPE */
        }

        let dt = self.new_node(NodeType::Doctype)?;
        let nm = self.own(self.sl(n, nl))?;
        {
            let d = self.doc.node_mut(dt);
            d.local = nm;
            d.qname = nm;
        }
        if let Some((ps, pl)) = pub_id {
            let p = self.own(self.sl(ps, pl))?;
            self.doc.node_mut(dt).prefix = p;
        }
        if let Some((ss, sl)) = sys_id {
            let s = self.own(self.sl(ss, sl))?;
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
        if self.starts(b"!--") {
            return self.parse_comment(parent);
        }
        if self.starts(b"![CDATA[") {
            return self.parse_cdata(parent);
        }
        if self.starts(b"!DOCTYPE") {
            if self.fragment.is_some() {
                return self.syntax(); /* a fragment has no DOCTYPE */
            }
            return self.parse_doctype();
        }
        self.syntax()
    }

    /// Start tag (cursor at the QName; tl/tc = the '<' position).
    fn parse_start_tag(&mut self, tl: u32, tc: u32) -> R {
        let (nm, nl) = self.scan_name()?;
        let name = self.sl(nm, nl);
        let sp = match split_scanned(name) {
            Some(s) => s,
            None => return self.syntax(),
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
                return self.syntax(); /* multiple roots */
            }
            self.doc.set_root(Some(el));
        }
        let parent = self.cur_parent();
        self.doc.append_child(parent, el);
        let bind_base = self.binds.len();
        let pushed = self.parse_element_body(el)?;
        if pushed {
            if self.stack.len() + 1 > MAX_DEPTH {
                return self.limit();
            }
            if self.stack.mkr_reserve(1).is_err() || self.frame.mkr_reserve(1).is_err() {
                self.status = Status::Oom;
                return Err(());
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
        let tstart = self.pos;
        let mut nonspace = false;
        while let Some(c) = self.peek() {
            if c == b'<' {
                break;
            }
            /* §2.4: the literal "]]>" must not appear in character data */
            if c == b']' && self.starts(b"]]>") {
                return self.syntax();
            }
            if !is_space(c) {
                nonspace = true;
            }
            self.advance();
        }
        let traw = self.pos - tstart;
        if traw > u32::MAX as usize {
            return self.limit();
        }
        if self.stack.is_empty() && self.fragment.is_none() {
            if nonspace {
                return self.syntax(); /* non-ws text outside any element */
            }
            return Ok(()); /* document-level whitespace: discarded */
        }
        let tv = self.expand(self.sl(tstart, traw), ExpandMode::Text)?;
        let parent = self.cur_parent();
        self.append_chardata(parent, NodeType::Text, tv)
    }

    /// Tokenizer dispatch.
    fn run(&mut self) {
        while self.left() > 0 {
            if self.peek() != Some(b'<') {
                if self.parse_text().is_err() {
                    break;
                }
            } else {
                let (tl, tc) = (self.line, self.col);
                let at_start = self.pos == 0 && self.fragment.is_none();
                self.advance(); /* '<' */
                let c = match self.peek() {
                    Some(c) => c,
                    None => {
                        let _ = self.syntax::<()>();
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
            if self.status != Status::Ok {
                break;
            }
        }
    }

    /// Seed a fragment parser's scope with the document root's xmlns attributes.
    fn seed_doc_namespaces(&mut self) -> R {
        let Some(root) = self.doc.root() else {
            return Ok(());
        };
        let mut a = self.doc.attrs(root);
        while let Some(attr) = a {
            let bpfx = xmlns_prefix(self.doc.qname(attr)).map(|p| p.to_vec());
            if let Some(bpfx) = bpfx {
                let uri = self.doc.node(attr).value;
                self.push_binding(&bpfx, uri)?;
            }
            a = self.doc.next(attr);
        }
        Ok(())
    }
}

/// Parse already-bounded input into a fresh document.
pub fn parse_ex(src: &[u8], limits: Option<usize>) -> Result<Box<Document>, Status> {
    let mut doc = Document::create(limits, src.len())?;
    let norm = match normalize_newlines(src) {
        Ok(n) => n,
        Err(()) => return Err(Status::Oom),
    };
    let body: &[u8] = match &norm {
        Some(v) => v,
        None => src,
    };
    let mut p = Parser::new(body, &mut doc, None);
    p.run();
    if p.status == Status::Ok && (!p.stack.is_empty() || p.doc.root().is_none()) {
        let _ = p.syntax::<()>(); /* unclosed element(s) / no root */
    }
    let st = p.status;
    drop(p);
    if st != Status::Ok {
        return Err(st);
    }
    Ok(doc)
}

/// Parse a fragment into a live document's arena. The document reference and
/// input slice make the ownership preconditions explicit to Rust callers.
pub fn parse_fragment(
    doc: &mut Document,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<NodeId, Status> {
    if src.len() > doc.max_bytes {
        return Err(Status::Limit);
    }
    let frag = doc.new_node(NodeType::Fragment)?;
    let norm = normalize_newlines(src).map_err(|_| doc.status)?;
    let body: &[u8] = match &norm {
        Some(v) => v,
        None => src,
    };
    let mut p = Parser::new(body, doc, Some(frag));
    if inherit_doc_ns && p.seed_doc_namespaces().is_err() {
        return Err(p.status);
    }
    p.run();
    if p.status == Status::Ok && !p.stack.is_empty() {
        let _ = p.syntax::<()>(); /* unclosed element(s) */
    }
    if p.status != Status::Ok {
        return Err(p.status); /* the partial fragment stays detached in the arena */
    }
    Ok(frag)
}
