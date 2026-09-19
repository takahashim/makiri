//! Tokenizer + tree builder (mkr_xml_tree.c). The scanning is safe slice code
//! over the input; the tree is an index arena, so this module contains no
//! `unsafe`.

#![forbid(unsafe_code)]

use crate::falloc::Reserve;
use crate::xml::chars::{
    decode1, is_name_char, is_name_start, is_reserved_pi_target, normalize_newlines,
    validate_chars, validate_name, ExpandMode,
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

/// An ExternalID's (public id, system id), each an (offset, len) input slice.
type ExternalId = (Option<(usize, usize)>, Option<(usize, usize)>);

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
    /// The general entities the internal subset declares, as input slices. A
    /// reference to one is a construct Makiri does not expand, not an
    /// undeclared name, and is reported as such.
    ge_names: Vec<(usize, usize)>,
    /// The DOCTYPE names an external subset, which a non-validating parser does
    /// not read; an entity it declares is equally not expanded.
    external_subset: bool,
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
            ge_names: Vec::new(),
            external_subset: false,
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
    /// Well-formed, but uses a DTD construct Makiri refuses rather than
    /// silently ignores (see `parse_int_subset`).
    #[inline]
    fn unsupported<T>(&mut self) -> R<T> {
        self.status = Status::Unsupported;
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
        if r == Err(Status::Syntax) && self.refs_unexpanded_entity(s) {
            return self.unsupported();
        }
        self.arena(r).map_err(|_| ())
    }

    /// Whether `s` references a general entity that a DTD declares (or may
    /// declare, in an external subset) - one Makiri does not expand, as opposed
    /// to an undeclared name, which is a well-formedness error.
    fn refs_unexpanded_entity(&self, s: &[u8]) -> bool {
        let mut i = 0;
        while let Some(at) = find(&s[i..], b'&') {
            i += at + 1;
            if s.get(i) == Some(&b'#') {
                continue;
            }
            let Some(end) = find(&s[i..], b';') else {
                return false;
            };
            let name = &s[i..i + end];
            if !matches!(name, b"lt" | b"gt" | b"amp" | b"apos" | b"quot")
                && (self.external_subset
                    || self.ge_names.iter().any(|&(o, l)| self.sl(o, l) == name))
            {
                return true;
            }
            i += end;
        }
        false
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
        /* §2.8: any 1.x. A 1.0 processor reads a 1.x document as 1.0, so one
         * that uses a 1.1-only feature fails on that feature, not its label. */
        if !is_version_num(ver) {
            return self.syntax();
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
        if tgt.contains(&b':') {
            return self.syntax(); /* Namespaces in XML §7: a PI target is an NCName */
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

    /// '<!DOCTYPE' (cursor at '!').
    ///
    /// The internal subset is parsed, not skipped: §5.1 requires even a
    /// non-validating processor to check it for well-formedness. Makiri does
    /// not APPLY what it declares, so a declaration that would change the tree
    /// fails the parse instead of being silently ignored (`parse_int_subset`).
    /// An external subset is named but, as §5.1 allows, never read.
    fn parse_doctype(&mut self) -> R {
        if !self.stack.is_empty() || self.doc.root().is_some() || self.saw_doctype {
            return self.syntax();
        }
        self.saw_doctype = true;
        self.advance_n(8); /* "!DOCTYPE" */
        self.need_space()?;
        self.skip_ws();
        let (n, nl) = self.scan_qname()?;

        let mut ids: ExternalId = (None, None);
        if self.peek().is_some_and(is_space) {
            self.skip_ws();
            if self.starts(b"SYSTEM") || self.starts(b"PUBLIC") {
                ids = self.scan_external_id(false)?;
                self.skip_ws();
            }
        }
        let (pub_id, sys_id) = ids;
        self.external_subset = sys_id.is_some();

        let unsupported = if self.peek() == Some(b'[') {
            self.advance();
            let u = self.parse_int_subset()?;
            self.advance(); /* ']' */
            self.skip_ws();
            u
        } else {
            false
        };
        if self.peek() != Some(b'>') {
            return self.syntax(); /* unterminated DOCTYPE */
        }
        self.advance();
        if unsupported {
            return self.unsupported();
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

    /* ---- the internal DTD subset (§2.8, §3.2-3.3, §4.2, §4.7) ---- */

    /// `intSubset` up to (not past) its closing ']'. Answers whether it holds a
    /// declaration Makiri refuses to ignore: one that would change the tree if
    /// applied - an attribute default or a non-CDATA attribute type (both
    /// change attribute values, §3.3.2-3.3.3) - or a parameter-entity reference,
    /// whose replacement text could carry either. The whole subset is still
    /// checked first, so a malformed one reports as malformed.
    ///
    /// Entity declarations are accepted: declaring one changes nothing until a
    /// reference to it, and that reference is refused where it occurs.
    fn parse_int_subset(&mut self) -> R<bool> {
        let mut unsupported = false;
        loop {
            self.skip_ws();
            match self.peek() {
                None => return self.syntax(),
                Some(b']') => return Ok(unsupported),
                Some(b'%') => {
                    /* DeclSep: a PEReference. */
                    self.advance();
                    let (s, l) = self.scan_name()?;
                    self.check_ncname(s, l)?;
                    if self.peek() != Some(b';') {
                        return self.syntax();
                    }
                    self.advance();
                    unsupported = true;
                }
                Some(b'<') => {
                    if self.starts(b"<!--") {
                        self.advance();
                        self.scan_comment()?;
                    } else if self.starts(b"<?") {
                        self.advance();
                        self.scan_subset_pi()?;
                    } else if self.eat_decl(b"<!ELEMENT")? {
                        self.parse_element_decl()?;
                    } else if self.eat_decl(b"<!ATTLIST")? {
                        unsupported |= self.parse_attlist_decl()?;
                    } else if self.eat_decl(b"<!ENTITY")? {
                        self.parse_entity_decl()?;
                    } else if self.eat_decl(b"<!NOTATION")? {
                        self.parse_notation_decl()?;
                    } else {
                        return self.syntax();
                    }
                }
                Some(_) => return self.syntax(),
            }
        }
    }

    /// A declaration keyword, which must be followed by white space.
    fn eat_decl(&mut self, kw: &[u8]) -> R<bool> {
        if !self.starts(kw) {
            return Ok(false);
        }
        self.advance_n(kw.len());
        self.need_space()?;
        self.skip_ws();
        Ok(true)
    }

    /// White space, then the declaration's closing '>'.
    fn end_decl(&mut self) -> R {
        self.skip_ws();
        if self.peek() != Some(b'>') {
            return self.syntax();
        }
        self.advance();
        Ok(())
    }

    /// A Name that must also be a QName (Namespaces in XML §3: element and
    /// attribute names, the DOCTYPE's included).
    fn scan_qname(&mut self) -> R<(usize, usize)> {
        let (s, l) = self.scan_name()?;
        if split_scanned(self.sl(s, l)).is_none() {
            return self.syntax();
        }
        Ok((s, l))
    }

    /// Namespaces in XML §7: every other Name - entity, notation, PI target -
    /// is an NCName.
    fn check_ncname(&mut self, s: usize, l: usize) -> R {
        if self.sl(s, l).contains(&b':') {
            return self.syntax();
        }
        Ok(())
    }

    /// '<!--' Comment '-->' in the subset (cursor at '!'); nothing is kept.
    fn scan_comment(&mut self) -> R {
        self.advance_n(3);
        let cstart = self.pos;
        let mut j = self.pos;
        loop {
            match find(&self.input[j..], b'-') {
                Some(at) => j += at,
                None => return self.syntax(),
            }
            if self.input.get(j + 1) == Some(&b'-') {
                if self.input.get(j + 2) == Some(&b'>') {
                    break;
                }
                return self.syntax();
            }
            j += 1;
        }
        if !validate_chars(&self.input[cstart..j]) {
            return self.syntax();
        }
        self.advance_n(j - cstart + 3);
        Ok(())
    }

    /// '<?' PI '?>' in the subset (cursor at '?'); nothing is kept.
    fn scan_subset_pi(&mut self) -> R {
        self.advance();
        let (t, tl) = self.scan_name()?;
        if is_reserved_pi_target(self.sl(t, tl)) {
            return self.syntax();
        }
        self.check_ncname(t, tl)?;
        if !self.starts(b"?>") {
            self.need_space()?;
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
        if !validate_chars(&self.input[dstart..j]) {
            return self.syntax();
        }
        self.advance_n(j - dstart + 2);
        Ok(())
    }

    /// A quoted literal whose characters must all be XML Chars.
    fn scan_char_literal(&mut self) -> R<(usize, usize)> {
        let (s, l) = self.parse_quoted()?;
        if !validate_chars(self.sl(s, l)) {
            return self.syntax();
        }
        Ok((s, l))
    }

    /// PubidLiteral (§2.3): a restricted ASCII set.
    fn scan_pubid_literal(&mut self) -> R<(usize, usize)> {
        let (s, l) = self.parse_quoted()?;
        let ok = self
            .sl(s, l)
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || b" \r\n-'()+,./:=?;!*#@$_%".contains(&c));
        if !ok {
            return self.syntax();
        }
        Ok((s, l))
    }

    /// ExternalID (§4.2.2), or with `public_only_ok` also NOTATION's PublicID:
    /// 'SYSTEM' S SystemLiteral | 'PUBLIC' S PubidLiteral (S SystemLiteral)?.
    /// Answers (public id, system id).
    fn scan_external_id(&mut self, public_only_ok: bool) -> R<ExternalId> {
        if self.eat_keyword(b"SYSTEM") {
            self.need_space()?;
            self.skip_ws();
            return Ok((None, Some(self.scan_char_literal()?)));
        }
        if !self.eat_keyword(b"PUBLIC") {
            return self.syntax();
        }
        self.need_space()?;
        self.skip_ws();
        let p = self.scan_pubid_literal()?;
        let before = self.pos;
        self.skip_ws();
        if matches!(self.peek(), Some(b'"' | b'\'')) && self.pos > before {
            return Ok((Some(p), Some(self.scan_char_literal()?)));
        }
        if public_only_ok {
            return Ok((Some(p), None));
        }
        self.syntax()
    }

    /// An EntityValue / AttValue literal: Chars, with every '&' a well-formed
    /// Reference. An AttValue may not hold '<'; an EntityValue may not hold
    /// '%', since in the internal subset a parameter-entity reference may not
    /// occur inside a declaration (WFC: PEs in Internal Subset). In an AttValue
    /// '%' is an ordinary character.
    fn scan_ref_literal(&mut self, att_value: bool) -> R {
        let (s, l) = self.scan_char_literal()?;
        let v = self.sl(s, l);
        let mut i = 0;
        while i < v.len() {
            match v[i] {
                b'%' if !att_value => return self.syntax(),
                b'<' if att_value => return self.syntax(),
                b'&' => {
                    let Some(end) = find(&v[i..], b';') else {
                        return self.syntax();
                    };
                    let body = &v[i + 1..i + end];
                    let ok = match body.strip_prefix(b"#") {
                        Some(num) => match num.strip_prefix(b"x") {
                            Some(hex) => !hex.is_empty() && hex.iter().all(u8::is_ascii_hexdigit),
                            None => !num.is_empty() && num.iter().all(u8::is_ascii_digit),
                        },
                        None => validate_name(body),
                    };
                    if !ok {
                        return self.syntax();
                    }
                    i += end;
                }
                _ => {}
            }
            i += 1;
        }
        Ok(())
    }

    /// elementdecl (§3.2), after '<!ELEMENT' S.
    fn parse_element_decl(&mut self) -> R {
        self.scan_qname()?;
        self.need_space()?;
        self.skip_ws();
        if !(self.eat_keyword(b"EMPTY") || self.eat_keyword(b"ANY")) {
            if self.peek() != Some(b'(') {
                return self.syntax();
            }
            self.advance();
            self.skip_ws();
            if self.eat_keyword(b"#PCDATA") {
                self.parse_mixed()?;
            } else {
                self.parse_cp_group(0)?;
                self.eat_quantifier();
            }
        }
        self.end_decl()
    }

    /// Mixed (§3.2.2), after '(' S? '#PCDATA'.
    fn parse_mixed(&mut self) -> R {
        let mut names = false;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.advance();
                    /* with names the '*' is required; bare #PCDATA may take one */
                    if self.peek() == Some(b'*') {
                        self.advance();
                    } else if names {
                        return self.syntax();
                    }
                    return Ok(());
                }
                Some(b'|') => {
                    self.advance();
                    self.skip_ws();
                    self.scan_qname()?;
                    names = true;
                }
                _ => return self.syntax(),
            }
        }
    }

    /// A choice or seq (§3.2.1) after its '(' - one kind of separator
    /// throughout. Nesting is bounded like element nesting, so a hostile
    /// content model cannot exhaust the stack.
    fn parse_cp_group(&mut self, depth: usize) -> R {
        if depth >= MAX_DEPTH {
            return self.limit();
        }
        let mut sep: Option<u8> = None;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'(') {
                self.advance();
                self.parse_cp_group(depth + 1)?;
            } else {
                self.scan_qname()?;
            }
            self.eat_quantifier();
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.advance();
                    return Ok(());
                }
                Some(c @ (b'|' | b',')) if sep.is_none_or(|s| s == c) => {
                    sep = Some(c);
                    self.advance();
                }
                _ => return self.syntax(),
            }
        }
    }

    fn eat_quantifier(&mut self) {
        if matches!(self.peek(), Some(b'?' | b'*' | b'+')) {
            self.advance();
        }
    }

    /// AttlistDecl (§3.3), after '<!ATTLIST' S. Answers whether any AttDef is
    /// one Makiri refuses: a non-CDATA type, or a default value.
    fn parse_attlist_decl(&mut self) -> R<bool> {
        self.scan_qname()?;
        let mut unsupported = false;
        loop {
            let before = self.pos;
            self.skip_ws();
            if self.peek() == Some(b'>') {
                self.advance();
                return Ok(unsupported);
            }
            if self.pos == before {
                return self.syntax(); /* AttDef ::= S Name ... */
            }
            self.scan_qname()?;
            self.need_space()?;
            self.skip_ws();
            let cdata = self.parse_att_type()?;
            self.need_space()?;
            self.skip_ws();
            let defaulted = if self.eat_keyword(b"#REQUIRED") || self.eat_keyword(b"#IMPLIED") {
                false
            } else {
                if self.eat_keyword(b"#FIXED") {
                    self.need_space()?;
                    self.skip_ws();
                }
                self.scan_ref_literal(true)?;
                true
            };
            unsupported |= !cdata || defaulted;
        }
    }

    /// AttType (§3.3.1); answers whether it is CDATA.
    fn parse_att_type(&mut self) -> R<bool> {
        if self.eat_keyword(b"CDATA") {
            return Ok(true);
        }
        /* longest first, so IDREFS is not read as ID + "REFS" */
        for kw in [
            &b"IDREFS"[..],
            b"IDREF",
            b"ID",
            b"ENTITIES",
            b"ENTITY",
            b"NMTOKENS",
            b"NMTOKEN",
        ] {
            if self.eat_keyword(kw) {
                return Ok(false);
            }
        }
        let notation = self.eat_keyword(b"NOTATION");
        if notation {
            self.need_space()?;
            self.skip_ws();
        }
        if self.peek() != Some(b'(') {
            return self.syntax();
        }
        self.advance();
        loop {
            self.skip_ws();
            if notation {
                let (s, l) = self.scan_name()?;
                self.check_ncname(s, l)?;
            } else {
                self.scan_nmtoken()?;
            }
            self.skip_ws();
            match self.peek() {
                Some(b')') => {
                    self.advance();
                    return Ok(false);
                }
                Some(b'|') => self.advance(),
                _ => return self.syntax(),
            }
        }
    }

    /// Nmtoken (§2.3): one or more NameChars.
    fn scan_nmtoken(&mut self) -> R {
        let start = self.pos;
        while let Some((cp, bl)) = decode1(&self.input[self.pos..]) {
            if !is_name_char(cp) {
                break;
            }
            self.advance_n(bl);
        }
        if self.pos == start {
            return self.syntax();
        }
        Ok(())
    }

    /// EntityDecl (§4.2), after '<!ENTITY' S. A general entity's name is
    /// recorded, so a reference to it reports as unexpanded, not undeclared.
    fn parse_entity_decl(&mut self) -> R {
        let pe = self.peek() == Some(b'%');
        if pe {
            self.advance();
            self.need_space()?;
            self.skip_ws();
        }
        let (s, l) = self.scan_name()?;
        self.check_ncname(s, l)?;
        self.need_space()?;
        self.skip_ws();
        if matches!(self.peek(), Some(b'"' | b'\'')) {
            self.scan_ref_literal(false)?;
        } else {
            self.scan_external_id(false)?;
            if !pe {
                /* NDataDecl ::= S 'NDATA' S Name */
                let before = self.pos;
                self.skip_ws();
                if self.pos > before && self.eat_keyword(b"NDATA") {
                    self.need_space()?;
                    self.skip_ws();
                    let (ns, nl) = self.scan_name()?;
                    self.check_ncname(ns, nl)?;
                }
            }
        }
        self.end_decl()?;
        if !pe {
            if self.ge_names.mkr_reserve(1).is_err() {
                self.status = Status::Oom;
                return Err(());
            }
            self.ge_names.push((s, l));
        }
        Ok(())
    }

    /// NotationDecl (§4.7), after '<!NOTATION' S.
    fn parse_notation_decl(&mut self) -> R {
        let (s, l) = self.scan_name()?;
        self.check_ncname(s, l)?;
        self.need_space()?;
        self.skip_ws();
        self.scan_external_id(true)?;
        self.end_decl()
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
