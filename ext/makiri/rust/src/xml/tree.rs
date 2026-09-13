//! Tokenizer + tree builder (mkr_xml_tree.c). The scanning is safe slice code
//! over the input; the unsafe blocks are confined to arena allocation and to
//! linking / reading the C-layout nodes.

use crate::falloc::Reserve;
use crate::xml::arena::{arena_node, doc_destroy, doc_new, ParserArena};
use crate::xml::chars::{
    decode1, is_name_char, is_name_start, is_reserved_pi_target, normalize_newlines,
    validate_chars, ExpandMode,
};
use crate::xml::qname::{
    is_enc_name, is_version_num, is_yes_no, split_scanned, xmlns_prefix, Split,
};
use crate::xml::{
    bytes, empty, node_local, node_prefix, node_qname, Doc, Node, ERR_LIMIT, ERR_OOM, ERR_SYNTAX,
    ERR_VERSION, FLAG_NS_RESOLVED, MAX_ATTRS, MAX_DEPTH, MAX_NS, OK, T_ATTRIBUTE, T_CDATA,
    T_COMMENT, T_DOCTYPE, T_DOCUMENT, T_ELEMENT, T_FRAGMENT, T_PI, T_TEXT, XMLNS_NS_URI,
    XML_NS_URI,
};
use core::ffi::c_char;
use core::ptr::{self, NonNull};

/// A namespace binding in scope: prefix ("" = default) -> arena-owned URI.
struct Binding {
    pfx: Vec<u8>,
    uri: *const c_char,
    uri_len: u32,
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
    arena: ParserArena,
    fragment: *mut Node,
    pub status: i32,
    binds: Vec<Binding>,
    ratt: Vec<RawAttr>,
    stack: Vec<*mut Node>,
    frame: Vec<usize>,
    saw_doctype: bool,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8], doc: NonNull<Doc>, fragment: *mut Node) -> Self {
        Parser {
            input,
            pos: 0,
            line: 1,
            col: 1,
            arena: ParserArena::new(doc),
            fragment,
            status: OK,
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
        if self.status == OK {
            self.status = ERR_SYNTAX;
        }
        Err(())
    }
    #[inline]
    fn limit<T>(&mut self) -> R<T> {
        self.status = ERR_LIMIT;
        Err(())
    }
    /// Propagate an arena failure recorded on the doc, then fail.
    #[inline]
    fn oom<T>(&mut self) -> R<T> {
        let st = self.arena.status();
        if st != OK {
            self.status = st;
        }
        Err(())
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
    fn cur_parent(&self) -> *mut Node {
        match self.stack.last() {
            Some(&n) => n,
            None if !self.fragment.is_null() => self.fragment,
            None => self.arena.document_node(),
        }
    }

    /// Copy a slice into the arena (fails closed on budget / OOM).
    fn own(&mut self, s: &[u8]) -> R<*const c_char> {
        let p = self.arena.bytes(s);
        if p.is_null() {
            return self.oom();
        }
        Ok(p)
    }

    fn expand(&mut self, s: &[u8], mode: ExpandMode) -> R<(*const c_char, u32)> {
        match self.arena.expand(s, mode) {
            Ok(x) => Ok(x),
            Err(st) => {
                self.status = st;
                Err(())
            }
        }
    }

    fn new_node(&mut self, ty: u32) -> R<*mut Node> {
        let n = self.arena.node(ty);
        if n.is_null() {
            return self.oom();
        }
        Ok(n)
    }

    /// Append a TEXT / CDATA node, coalescing with a preceding sibling of the
    /// SAME type (as libxml2 / the XPath data model do).
    fn append_chardata(&mut self, parent: *mut Node, ty: u32, val: *const c_char, len: u32) -> R {
        match self.arena.append_chardata(parent, ty, val, len) {
            Ok(()) => Ok(()),
            Err(ERR_LIMIT) => self.limit(),
            Err(_) => self.oom(),
        }
    }

    /// Store `name` (prefix:local per `sp`) as one arena copy on `node`.
    fn set_node_qname(&mut self, node: *mut Node, name: &[u8], sp: &Split) -> R {
        let qn = crate::xml::qname_from(name, sp);
        if self.arena.assign_qname(node, &qn) != 0 {
            return self.oom();
        }
        Ok(())
    }

    /* ---- namespaces (§7) ---- */

    fn ns_lookup(&self, pfx: &[u8]) -> Option<(*const c_char, u32)> {
        if pfx == b"xml" {
            return Some((
                XML_NS_URI.as_ptr() as *const c_char,
                XML_NS_URI.len() as u32,
            ));
        }
        self.binds
            .iter()
            .rev()
            .find(|b| b.pfx == pfx)
            .map(|b| (b.uri, b.uri_len))
    }

    fn push_binding(&mut self, pfx: &[u8], uri: *const c_char, uri_len: u32) -> R {
        if self.binds.len() + 1 > MAX_NS {
            return self.limit();
        }
        let mut v: Vec<u8> = Vec::new();
        if v.mkr_reserve_exact(pfx.len()).is_err() || self.binds.mkr_reserve(1).is_err() {
            self.status = ERR_OOM;
            return Err(());
        }
        v.extend_from_slice(pfx);
        self.binds.push(Binding {
            pfx: v,
            uri,
            uri_len,
        });
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
                self.status = ERR_OOM;
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
            let (uri, ulen) = self.expand(&input[r.val.0..r.val.0 + r.val.1], ExpandMode::Attr)?;
            let u = unsafe { bytes(uri, ulen) };
            if bpfx == b"xml" {
                if u != XML_NS_URI {
                    return self.syntax();
                }
            } else if u == XML_NS_URI || u == XMLNS_NS_URI {
                return self.syntax(); /* reserved URI bound to another prefix */
            }
            if !bpfx.is_empty() && ulen == 0 {
                return self.syntax(); /* xmlns:p="" (XML 1.0) */
            }
            self.push_binding(bpfx, uri, ulen)?;
        }
        Ok(())
    }

    /// Phase 3: the element's own namespace URI.
    fn resolve_element_ns(&mut self, el: *mut Node) -> R {
        unsafe {
            if (*el).prefix_len > 0 {
                let pfx = node_prefix(el);
                if pfx == b"xmlns" {
                    return self.syntax();
                }
                match self.ns_lookup(pfx) {
                    Some((u, l)) => {
                        (*el).ns_uri = u;
                        (*el).ns_uri_len = l;
                    }
                    None => return self.syntax(), /* unbound prefix */
                }
            } else if let Some((u, l)) = self.ns_lookup(b"") {
                if l > 0 {
                    (*el).ns_uri = u;
                    (*el).ns_uri_len = l;
                }
            }
            /* Decided: from here the URI is the node's identity (lib.rs). A
             * parsed element in no namespace is resolved too - "no namespace" is
             * a decision, not an absence, and moving it under a default
             * namespace must not silently put it in one. */
            (*el).flags |= FLAG_NS_RESOLVED;
        }
        Ok(())
    }

    /// Phase 4: the attribute nodes (xmlns kept, §7.2), then duplicates (§9.3).
    fn build_attr_nodes(&mut self, el: *mut Node) -> R {
        let input = self.input;
        let mut tail: *mut Node = ptr::null_mut();
        for i in 0..self.ratt.len() {
            let r = self.ratt[i];
            let name = &input[r.name.0..r.name.0 + r.name.1];
            let sp = match split_scanned(name) {
                Some(s) => s,
                None => return self.syntax(),
            };
            let attr = self.new_node(T_ATTRIBUTE)?;
            self.set_node_qname(attr, name, &sp)?;
            unsafe {
                if xmlns_prefix(name).is_some() {
                    (*attr).ns_uri = XMLNS_NS_URI.as_ptr() as *const c_char;
                    (*attr).ns_uri_len = XMLNS_NS_URI.len() as u32;
                } else if sp.prefix_len > 0 {
                    match self.ns_lookup(&name[..sp.prefix_len as usize]) {
                        Some((u, l)) => {
                            (*attr).ns_uri = u;
                            (*attr).ns_uri_len = l;
                        }
                        None => return self.syntax(), /* unbound prefix */
                    }
                }
                let (v, vl) = self.expand(&input[r.val.0..r.val.0 + r.val.1], ExpandMode::Attr)?;
                (*attr).value = v;
                (*attr).value_len = vl;
                (*attr).parent = el;
                if tail.is_null() {
                    (*el).attrs = attr;
                } else {
                    (*tail).next = attr;
                }
                tail = attr;
            }
        }
        /* §9.3: no two attributes share (namespace URI, local name) */
        unsafe {
            let mut a = (*el).attrs;
            while !a.is_null() {
                let mut b = (*a).next;
                while !b.is_null() {
                    if node_local(a) == node_local(b)
                        && crate::xml::node_ns(a) == crate::xml::node_ns(b)
                    {
                        return self.syntax();
                    }
                    b = (*b).next;
                }
                a = (*a).next;
            }
        }
        Ok(())
    }

    fn parse_element_body(&mut self, el: *mut Node) -> R<bool> {
        let pushed = self.scan_raw_attrs()?;
        self.apply_xmlns_bindings()?;
        self.resolve_element_ns(el)?;
        self.build_attr_nodes(el)?;
        Ok(pushed)
    }

    /* ---- markup ---- */

    /// '<!--' comment (cursor at '!').
    fn parse_comment(&mut self, parent: *mut Node) -> R {
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
        if !parent.is_null() {
            let c = self.new_node(T_COMMENT)?;
            let v = self.own(self.sl(cstart, craw))?;
            unsafe {
                (*c).value = v;
                (*c).value_len = craw as u32;
                self.arena.append(parent, c);
            }
        }
        self.advance_n(craw + 3);
        Ok(())
    }

    /// '<![CDATA[' section (cursor at '!').
    fn parse_cdata(&mut self, parent: *mut Node) -> R {
        if self.stack.is_empty() && self.fragment.is_null() {
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
        self.append_chardata(parent, T_CDATA, cval, craw as u32)?;
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
            self.status = ERR_VERSION; /* well-formed, unsupported version */
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
                self.arena.mark_encoding_decl();
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
    fn parse_pi(&mut self, parent: *mut Node, at_doc_start: bool) -> R {
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
        if !parent.is_null() {
            let pi = self.new_node(T_PI)?;
            let lp = self.own(tgt)?;
            let vp = self.own(self.sl(dstart, draw))?;
            unsafe {
                (*pi).local = lp;
                (*pi).local_len = tl as u32;
                (*pi).value = vp;
                (*pi).value_len = draw as u32;
                self.arena.append(parent, pi);
            }
        }
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
        let matched = unsafe {
            if (*top).prefix_len > 0 {
                let pl = (*top).prefix_len as usize;
                let tql = pl + 1 + (*top).local_len as usize;
                nl == tql
                    && name.starts_with(node_prefix(top))
                    && name[pl] == b':'
                    && &name[pl + 1..] == node_local(top)
            } else {
                name == node_local(top)
            }
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
        if !self.stack.is_empty() || !self.arena.root().is_null() || self.saw_doctype {
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

        let dt = self.new_node(T_DOCTYPE)?;
        let nm = self.own(self.sl(n, nl))?;
        unsafe {
            (*dt).local = nm;
            (*dt).qname = nm;
            (*dt).local_len = nl as u32;
            (*dt).qname_len = nl as u32;
        }
        if let Some((ps, pl)) = pub_id {
            let p = self.own(self.sl(ps, pl))?;
            unsafe {
                (*dt).prefix = p;
                (*dt).prefix_len = pl as u32;
            }
        }
        if let Some((ss, sl)) = sys_id {
            let s = self.own(self.sl(ss, sl))?;
            unsafe {
                (*dt).value = s;
                (*dt).value_len = sl as u32;
            }
        }
        self.arena.append(self.arena.document_node(), dt);
        self.arena.set_doctype(dt);
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
            if !self.fragment.is_null() {
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
        let el = self.new_node(T_ELEMENT)?;
        self.set_node_qname(el, name, &sp)?;
        unsafe {
            (*el).line = tl;
            (*el).col = tc;
            if self.stack.is_empty() && self.fragment.is_null() {
                if !self.arena.root().is_null() {
                    return self.syntax(); /* multiple roots */
                }
                self.arena.set_root(el);
            }
            self.arena.append(self.cur_parent(), el);
        }
        let bind_base = self.binds.len();
        let pushed = self.parse_element_body(el)?;
        if pushed {
            if self.stack.len() + 1 > MAX_DEPTH {
                return self.limit();
            }
            if self.stack.mkr_reserve(1).is_err() || self.frame.mkr_reserve(1).is_err() {
                self.status = ERR_OOM;
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
        if self.stack.is_empty() && self.fragment.is_null() {
            if nonspace {
                return self.syntax(); /* non-ws text outside any element */
            }
            return Ok(()); /* document-level whitespace: discarded */
        }
        let (tv, tvl) = self.expand(self.sl(tstart, traw), ExpandMode::Text)?;
        let parent = self.cur_parent();
        self.append_chardata(parent, T_TEXT, tv, tvl)
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
                let at_start = self.pos == 0 && self.fragment.is_null();
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
            if self.status != OK {
                break;
            }
        }
    }

    /// Seed a fragment parser's scope with the document root's xmlns attributes.
    fn seed_doc_namespaces(&mut self) -> R {
        let root = self.arena.root();
        if root.is_null() {
            return Ok(());
        }
        let mut a = unsafe { (*root).attrs };
        while !a.is_null() {
            let (qn, v, vl) = unsafe {
                let v = if (*a).value.is_null() {
                    empty()
                } else {
                    (*a).value
                };
                (node_qname(a), v, (*a).value_len)
            };
            if let Some(bpfx) = xmlns_prefix(qn) {
                self.push_binding(bpfx, v, vl)?;
            }
            a = unsafe { (*a).next };
        }
        Ok(())
    }
}

/// Parse already-bounded input into a fresh document. Raw input is converted
/// to this slice at the FFI boundary, before reaching the tree builder.
pub fn parse_ex(src: &[u8], limits: Option<usize>) -> Result<*mut Doc, i32> {
    // SAFETY: this function creates the document before handing its sole raw
    // pointer to the builder; `src` is an ordinary Rust slice.
    unsafe { parse_ex_in(src, limits) }
}

unsafe fn parse_ex_in(src: &[u8], limits: Option<usize>) -> Result<*mut Doc, i32> {
    let doc = doc_new();
    if doc.is_null() {
        return Err(ERR_OOM);
    }
    if let Some(mb) = limits {
        if mb != 0 {
            (*doc).max_bytes = mb;
        }
    }
    if src.len() > (*doc).max_bytes {
        doc_destroy(doc);
        return Err(ERR_LIMIT);
    }
    (*doc).doc_node = arena_node(doc, T_DOCUMENT);
    if (*doc).doc_node.is_null() {
        let st = (*doc).oom;
        doc_destroy(doc);
        return Err(st);
    }
    let norm = match normalize_newlines(src) {
        Ok(n) => n,
        Err(()) => {
            doc_destroy(doc);
            return Err(ERR_OOM);
        }
    };
    let body: &[u8] = match &norm {
        Some(v) => v,
        None => src,
    };
    let mut p = Parser::new(body, NonNull::new_unchecked(doc), ptr::null_mut());
    p.run();
    if p.status == OK && (!p.stack.is_empty() || (*doc).root.is_null()) {
        let _ = p.syntax::<()>(); /* unclosed element(s) / no root */
    }
    let st = p.status;
    drop(p);
    if st != OK {
        doc_destroy(doc);
        return Err(st);
    }
    Ok(doc)
}

/// Compatibility shim for the raw self-test harness. Production callers use
/// [`parse_ex`] through `ffi.rs`, where the pointer boundary belongs.
///
/// # Safety
/// `src` must name `len` readable bytes unless `len` exceeds the requested
/// limit. The length check deliberately precedes the slice conversion.
pub unsafe fn parse_ex_raw(
    src: *const c_char,
    len: usize,
    limits: Option<usize>,
) -> Result<*mut Doc, i32> {
    let max = limits.filter(|&n| n != 0).unwrap_or(crate::xml::MAX_BYTES);
    if len > max {
        return Err(ERR_LIMIT);
    }
    let src = if src.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(src as *const u8, len)
    };
    parse_ex(src, limits)
}

/// Parse a fragment into a live document's arena. The document reference and
/// input slice make the ownership preconditions explicit to Rust callers.
pub fn parse_fragment(doc: &mut Doc, src: &[u8], inherit_doc_ns: bool) -> Result<*mut Node, i32> {
    let doc = doc as *mut Doc;
    unsafe { parse_fragment_in(doc, src, inherit_doc_ns) }
}

/// Raw self-test compatibility shim; production FFI converts its arguments
/// before entering the tree builder.
///
/// # Safety
/// `doc` must be live and `src` must name `len` readable bytes.
pub unsafe fn parse_fragment_raw(
    doc: *mut Doc,
    src: *const c_char,
    len: usize,
    inherit_doc_ns: bool,
) -> Result<*mut Node, i32> {
    if doc.is_null() || len > (*doc).max_bytes {
        return Err(ERR_LIMIT);
    }
    let src = if src.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(src as *const u8, len)
    };
    parse_fragment_in(doc, src, inherit_doc_ns)
}

unsafe fn parse_fragment_in(
    doc: *mut Doc,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<*mut Node, i32> {
    if src.len() > (*doc).max_bytes {
        return Err(ERR_LIMIT);
    }
    let frag = arena_node(doc, T_FRAGMENT);
    if frag.is_null() {
        return Err((*doc).oom);
    }
    let norm = normalize_newlines(src).map_err(|_| ERR_OOM)?;
    let body: &[u8] = match &norm {
        Some(v) => v,
        None => src,
    };
    let mut p = Parser::new(body, NonNull::new_unchecked(doc), frag);
    if inherit_doc_ns && p.seed_doc_namespaces().is_err() {
        return Err(p.status);
    }
    p.run();
    if p.status == OK && !p.stack.is_empty() {
        let _ = p.syntax::<()>(); /* unclosed element(s) */
    }
    if p.status != OK {
        return Err(p.status); /* the partial fragment stays detached in the arena */
    }
    Ok(frag)
}
