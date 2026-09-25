//! Tokenizer and tree builder: the XML entry points (`parse`, `parse_ex`,
//! `parse_fragment`) and the document they build.
//!
//! The scanning is safe slice code over the input and the tree is an index
//! arena, so this module contains no `unsafe`. Reading bytes is [`cursor`]'s
//! job and checking the internal DTD subset is [`dtd`]'s; what is left here is
//! the part that actually makes nodes.

#![forbid(unsafe_code)]

mod cursor;
mod decl;
mod dtd;
mod scope;

use crate::falloc::Reserve;
use crate::xml::chars::{is_reserved_pi_target, normalize_newlines, ExpandMode};
use crate::xml::qname::{split_scanned, xmlns_prefix, Split};
use crate::xml::{Document, Limits, NodeId, NodeType, Span, Status, MAX_ATTRS, MAX_DEPTH};
use cursor::{is_space, Cursor, InSlice, R};
use dtd::{scan_external_id, Declared, ExternalId, Subset};
use scope::{Frame, Scope, ScopeFull};

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
    scope: Scope,
    ratt: Vec<RawAttr>,
    stack: Vec<NodeId>,
    frames: Vec<Frame>,
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
            scope: Scope::default(),
            ratt: Vec::new(),
            stack: Vec::new(),
            frames: Vec::new(),
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
        if r == Err(Status::Syntax) && self.declared.refs_unexpanded_entity(&self.cur, s) {
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

    /// The in-scope URI for `pfx`. `xml` is bound without a declaration, and to
    /// a URI the DOCUMENT owns, which is why the scope itself cannot answer it.
    fn ns_lookup(&self, pfx: &[u8]) -> Option<Span> {
        if pfx == b"xml" {
            return Some(self.doc.xml_ns_span());
        }
        self.scope.lookup(pfx)
    }

    fn push_binding(&mut self, pfx: &[u8], uri: Span) -> R {
        bind(&mut self.scope, &mut self.cur, pfx, uri)
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
            let val = self.cur.slice(r.val);
            let uri = self.expand(val, ExpandMode::Attr)?;
            if crate::xml::qname::ns_decl_check(bpfx, self.doc.span(uri)).is_err() {
                return self.cur.syntax();
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
            self.doc.link_attr(el, tail, attr);
            tail = Some(attr);
        }
        /* §9.3: no two attributes share (namespace URI, local name) */
        match has_duplicate_attributes(self.doc, el) {
            Some(false) => {}
            Some(true) => return self.cur.syntax(),
            None => return self.cur.fail(Status::Oom),
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

    /// '<?' processing instruction (cursor at '?').
    fn parse_pi(&mut self, parent: NodeId, at_doc_start: bool) -> R {
        self.cur.advance_n(1);
        let t = self.cur.scan_name()?;
        let tgt = self.cur.slice(t);
        if tgt == b"xml" {
            if !at_doc_start || !self.stack.is_empty() {
                return self.cur.syntax();
            }
            /* `<?xml ...?>` is the declaration, not a PI: no node comes of it. */
            return decl::parse_body(&mut self.cur, self.doc);
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
        match self.frames.pop() {
            Some(frame) => self.scope.leave(frame),
            None => self.scope.leave_without_frame(),
        }
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
                ids = scan_external_id(&mut self.cur, false)?;
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

        let (public, system) = (
            ids.public.map(|p| self.cur.slice(p)),
            ids.system.map(|s| self.cur.slice(s)),
        );
        let r = self.doc.new_doctype(self.cur.slice(name), public, system);
        let dt = self.arena(r)?;
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
        let frame = self.scope.enter();
        let pushed = self.parse_element_body(el)?;
        if pushed {
            if self.stack.len() + 1 > MAX_DEPTH {
                return self.cur.limit();
            }
            if self.stack.falloc_reserve(1).is_err() || self.frames.falloc_reserve(1).is_err() {
                return self.cur.fail(Status::Oom);
            }
            self.stack.push(el);
            self.frames.push(frame);
        } else {
            self.scope.leave(frame); /* self-closing: pop its scope now */
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
        for attr in self.doc.attributes(root) {
            /* The prefix borrows `self.doc`, which `bind` does not touch, so it
             * is passed as is: `Scope::bind` makes its own copy. */
            if let Some(p) = xmlns_prefix(self.doc.qname(attr)) {
                let uri = self.doc.node(attr).value;
                bind(&mut self.scope, &mut self.cur, p, uri)?;
            }
        }
        Ok(())
    }
}

/// Bind `pfx` in `scope`, reporting a refusal through `cur`'s status.
///
/// A free function so a caller can bind a prefix that borrows the document
/// the parser holds.
fn bind(scope: &mut Scope, cur: &mut Cursor<'_>, pfx: &[u8], uri: Span) -> R {
    match scope.bind(pfx, uri) {
        Ok(()) => Ok(()),
        Err(ScopeFull::Limit) => cur.limit(),
        Err(ScopeFull::Oom) => cur.fail(Status::Oom),
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

/// XML §9.3: no two attributes of one element share a `(namespace URI, local
/// name)`. Whether two of `element`'s do - or None when the sort buffer cannot
/// be allocated.
///
/// A free function here rather than a `Document` method in `arena`: the arena
/// stores nodes, it does not judge whether they are well-formed. It reads
/// through the checked accessors, which is what a rule at this layer should do.
///
/// Pairwise for the usual handful. Past that, pairwise is quadratic in a count
/// the input picks, up to `MAX_ATTRS`: 8.4M comparisons an element, and 100
/// such elements (3.6 MB) took 16.6 s with no budget to stop it. So a longer
/// list is sorted by the pair and compared as neighbours, O(n log n).
fn has_duplicate_attributes(doc: &Document, element: NodeId) -> Option<bool> {
    const PAIRWISE_MAX: usize = 16;
    let attrs = || core::iter::successors(doc.attrs(element), |&x| doc.next(x));
    let count = attrs().count();
    if count <= PAIRWISE_MAX {
        let found = attrs().enumerate().any(|(i, x)| {
            attrs()
                .skip(i + 1)
                .any(|y| doc.local(x) == doc.local(y) && doc.ns(x) == doc.ns(y))
        });
        return Some(found);
    }
    let mut ids: Vec<NodeId> = Vec::new();
    ids.falloc_reserve_exact(count).ok()?;
    ids.extend(attrs());
    let key = |x: NodeId| (doc.ns(x), doc.local(x));
    ids.sort_unstable_by(|&x, &y| key(x).cmp(&key(y)));
    Some(ids.windows(2).any(|w| key(w[0]) == key(w[1])))
}
