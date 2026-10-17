//! The HTML DOM of the parser port: one document as an index arena.
//!
//! The successor of Lexbor's DOM (`docs/rewrite_parser.ja.md`, stage 2). It
//! keeps the observable model the Lexbor adapter (`lexbor::adapter::html`)
//! exposes - node kinds, the names an element or attribute reports, namespaces
//! and prefixes, attribute lists with their `id`/`class` shortcuts, template
//! contents, source offsets - and stores it as indices: a node is a [`NodeId`]
//! into one `Vec`, character data and written names are spans of one byte
//! store, and the per-kind fields live in side tables. Nothing here is
//! `unsafe`: an edit holds the document by `&mut`, a reader by `&`.
//!
//! Names follow Lexbor's split. An element's or attribute's LOCAL name is an
//! id in the document's [`Names`] (Lexbor's static ids, then appended ones,
//! ASCII-lower-cased); the name AS WRITTEN, when it differs (a prefixed name,
//! an SVG `foreignObject`), is kept beside it, as Lexbor's `qualified_name`
//! is. Namespaces and prefixes are tables of their own whose first entries are
//! Lexbor's static ids.

#![forbid(unsafe_code)]

use core::num::NonZeroU32;

use crate::falloc::{try_vec_with_capacity, Reserve, VecPush};
use crate::html::names::Names;
use crate::html::res::names as res;
pub use crate::node_type::NodeType;

/// An allocation the arena needed failed; the document is as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomOom;

/// A node of one document: its index + 1, so `Option<NodeId>` is four bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    /// The document node, always the first.
    pub const DOCUMENT: NodeId = NodeId(NonZeroU32::MIN);

    #[inline]
    fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }

    #[inline]
    fn from_index(i: usize) -> Option<NodeId> {
        u32::try_from(i + 1)
            .ok()
            .and_then(NonZeroU32::new)
            .map(NodeId)
    }

    /// The id as one word, for a table key or a Ruby-side handle.
    #[inline]
    pub fn to_word(self) -> usize {
        self.0.get() as usize
    }

    /// An id back from [`to_word`](Self::to_word); `None` for 0 or a word past
    /// `u32`.
    #[inline]
    pub fn from_word(w: usize) -> Option<NodeId> {
        u32::try_from(w).ok().and_then(NonZeroU32::new).map(NodeId)
    }
}

/// `[start, start + len)` of the byte store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    start: u32,
    len: u32,
}

/// A namespace id: Lexbor's static numbers (`res::NS_*`) first, then the
/// document's own. 0 (`NS__UNDEF`) is "none" and never stored.
pub type NsIndex = u32;
/// A prefix id, numbered as namespaces are (Lexbor shares the static range).
pub type PrefixIndex = u32;

#[derive(Clone, Copy, Debug)]
enum Payload {
    Document,
    Element(u32),
    Attr(u32),
    /// Text, comment or CDATA data.
    Data(Span),
    Pi {
        target: Span,
        data: Span,
    },
    Doctype(u32),
    Fragment {
        host: Option<NodeId>,
    },
}

#[derive(Clone, Copy, Debug)]
struct Node {
    kind: NodeType,
    /// Lexbor's `node.local_name`, for every kind of node: an element's tag
    /// id, an attribute's local-name id, the special tag id Lexbor's parser
    /// gives a text, comment or doctype node (`#text`, `!--`, ...), 0 where
    /// Lexbor leaves it unset.
    local: u32,
    /// Lexbor's `node.ns`: the namespace, 0 for none. An attribute in no
    /// namespace of its own carries its element's (see
    /// [`Document::attr_own_ns`]).
    ns: NsIndex,
    /// Lexbor's `node.prefix`, 0 for none.
    prefix: PrefixIndex,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    /// The tree siblings. An attribute has none: its element's list is
    /// linked through [`AttrData`], as Lexbor links `attr->next`.
    next: Option<NodeId>,
    prev: Option<NodeId>,
    /// The start tag's byte offset + 1; 0 when the node has none.
    source: u32,
    payload: Payload,
}

/// An element's own fields.
#[derive(Clone, Copy, Debug)]
pub struct ElementData {
    /// The qualified name as written, when Lexbor would keep one: an id of
    /// [`Names::qualified`], 0 for none.
    written: u32,
    first_attr: Option<NodeId>,
    last_attr: Option<NodeId>,
    /// The no-namespace `id` and `class` attributes (Lexbor's `attr_id` /
    /// `attr_class`), which CSS `#id` and `.class` read.
    id_attr: Option<NodeId>,
    class_attr: Option<NodeId>,
    /// An HTML `<template>`'s contents fragment.
    template: Option<NodeId>,
}

/// An attribute's own fields.
#[derive(Clone, Copy, Debug)]
pub struct AttrData {
    /// As [`ElementData`]'s.
    written: u32,
    value: Span,
    /// The next and previous attribute of its element's list.
    next: Option<NodeId>,
    prev: Option<NodeId>,
}

#[derive(Clone, Copy, Debug)]
struct DoctypeData {
    name: Span,
    public_id: Span,
    system_id: Span,
}

/// One HTML document.
pub struct Document {
    nodes: Vec<Node>,
    elements: Vec<ElementData>,
    attrs: Vec<AttrData>,
    doctypes: Vec<DoctypeData>,
    bytes: Vec<u8>,
    /// Tag and attribute names, shared with the tokenizer.
    pub names: Names,
    /// Namespace URIs past the static ones.
    ns: Vec<Vec<u8>>,
    /// Prefixes past the static ones.
    prefixes: Vec<Vec<u8>>,
    /// Lexbor's `compat_mode`: 0 no-quirks, 1 quirks, 2 limited-quirks.
    pub compat_mode: u8,
}

macro_rules! oom {
    ($e:expr) => {
        $e.map_err(|_| DomOom)?
    };
}

impl Document {
    /// An empty document in `compat_mode`.
    pub fn new(compat_mode: u8) -> Result<Document, DomOom> {
        let mut nodes: Vec<Node> = try_vec_with_capacity(16).ok_or(DomOom)?;
        nodes.push(Node {
            kind: NodeType::Document,
            local: res::TAG__DOCUMENT as u32,
            ns: 0,
            prefix: 0,
            parent: None,
            first_child: None,
            last_child: None,
            next: None,
            prev: None,
            source: 0,
            payload: Payload::Document,
        });
        Ok(Document {
            nodes,
            elements: Vec::new(),
            attrs: Vec::new(),
            doctypes: Vec::new(),
            bytes: Vec::new(),
            names: Names::default(),
            ns: Vec::new(),
            prefixes: Vec::new(),
            compat_mode: compat_mode.min(2),
        })
    }

    /// How many nodes the arena holds (live or detached).
    #[inline]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Bytes the document owns, for the GC report.
    pub fn owned_bytes(&self) -> usize {
        self.nodes.capacity() * core::mem::size_of::<Node>()
            + self.elements.capacity() * core::mem::size_of::<ElementData>()
            + self.attrs.capacity() * core::mem::size_of::<AttrData>()
            + self.doctypes.capacity() * core::mem::size_of::<DoctypeData>()
            + self.bytes.capacity()
            + self.names.owned_bytes()
    }

    /* ---- storing ---- */

    fn store(&mut self, b: &[u8]) -> Result<Span, DomOom> {
        let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
        let len = u32::try_from(b.len()).map_err(|_| DomOom)?;
        start.checked_add(len).ok_or(DomOom)?;
        oom!(self.bytes.falloc_reserve(b.len()));
        self.bytes.extend_from_slice(b);
        Ok(Span { start, len })
    }

    /// A second copy of the stored bytes `s`.
    fn dup(&mut self, s: Span) -> Result<Span, DomOom> {
        let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
        start.checked_add(s.len).ok_or(DomOom)?;
        let from = s.start as usize;
        let to = from + s.len as usize;
        if to > self.bytes.len() {
            return Err(DomOom);
        }
        oom!(self.bytes.falloc_reserve(s.len as usize));
        self.bytes.extend_from_within(from..to);
        Ok(Span { start, len: s.len })
    }

    #[inline]
    fn bytes_of(&self, s: Span) -> &[u8] {
        let start = s.start as usize;
        self.bytes
            .get(start..start + s.len as usize)
            .unwrap_or_default()
    }

    fn push(&mut self, kind: NodeType, payload: Payload) -> Result<NodeId, DomOom> {
        let id = NodeId::from_index(self.nodes.len()).ok_or(DomOom)?;
        oom!(self.nodes.falloc_push(Node {
            kind,
            local: 0,
            ns: 0,
            prefix: 0,
            parent: None,
            first_child: None,
            last_child: None,
            next: None,
            prev: None,
            source: 0,
            payload,
        }));
        Ok(id)
    }

    #[inline]
    fn node(&self, n: NodeId) -> Option<&Node> {
        self.nodes.get(n.index())
    }

    #[inline]
    fn node_mut(&mut self, n: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(n.index())
    }

    /* ---- namespaces and prefixes ---- */

    /// The id of the namespace `uri`, matched byte for byte; `None` when the
    /// document has none by that URI (or for an empty one).
    pub fn lookup_ns(&self, uri: &[u8]) -> Option<NsIndex> {
        if uri.is_empty() {
            return None;
        }
        if let Some(i) = res::NS_LINKS
            .iter()
            .skip(res::NS_HTML)
            .position(|l| *l == uri)
        {
            return u32::try_from(i + res::NS_HTML).ok();
        }
        self.ns
            .iter()
            .position(|l| l == uri)
            .and_then(|i| u32::try_from(res::NS__LAST_ENTRY + i).ok())
    }

    /// [`lookup_ns`](Self::lookup_ns), appending `uri` when new.
    pub fn intern_ns(&mut self, uri: &[u8]) -> Result<Option<NsIndex>, DomOom> {
        if uri.is_empty() {
            return Ok(None);
        }
        if let Some(i) = self.lookup_ns(uri) {
            return Ok(Some(i));
        }
        let owned = crate::falloc::try_to_vec(uri).ok_or(DomOom)?;
        oom!(self.ns.falloc_push(owned));
        Ok(u32::try_from(res::NS__LAST_ENTRY + self.ns.len() - 1).ok())
    }

    /// A namespace id's URI.
    pub fn ns_uri(&self, ns: NsIndex) -> Option<&[u8]> {
        let i = ns as usize;
        if i == 0 {
            return None;
        }
        match res::NS_LINKS.get(i) {
            Some(l) => (!l.is_empty()).then_some(*l),
            None => self.ns.get(i - res::NS__LAST_ENTRY).map(Vec::as_slice),
        }
    }

    /// The id of `prefix`, appending it when new; `None` for an empty one.
    pub fn intern_prefix(&mut self, prefix: &[u8]) -> Result<Option<PrefixIndex>, DomOom> {
        if prefix.is_empty() {
            return Ok(None);
        }
        if let Some(i) = res::NS_PREFIXES
            .iter()
            .skip(res::NS_HTML)
            .position(|p| *p == prefix)
        {
            return Ok(u32::try_from(i + res::NS_HTML).ok());
        }
        if let Some(i) = self.prefixes.iter().position(|p| p == prefix) {
            return Ok(u32::try_from(res::NS__LAST_ENTRY + i).ok());
        }
        let owned = crate::falloc::try_to_vec(prefix).ok_or(DomOom)?;
        oom!(self.prefixes.falloc_push(owned));
        Ok(u32::try_from(res::NS__LAST_ENTRY + self.prefixes.len() - 1).ok())
    }

    /// A prefix id's text.
    pub fn prefix_text(&self, p: PrefixIndex) -> Option<&[u8]> {
        let i = p as usize;
        if i == 0 {
            return None;
        }
        match res::NS_PREFIXES.get(i) {
            Some(t) => (!t.is_empty()).then_some(*t),
            None => self
                .prefixes
                .get(i - res::NS__LAST_ENTRY)
                .map(Vec::as_slice),
        }
    }

    /* ---- making nodes (detached) ---- */

    fn intern_written(&mut self, written: Option<&[u8]>) -> Result<u32, DomOom> {
        match written {
            Some(w) if !w.is_empty() => self.names.qualified_append(w).map_err(|_| DomOom),
            _ => Ok(0),
        }
    }

    /// Set the element's or attribute's qualified name as written (`None`:
    /// it has none of its own).
    pub fn set_written(&mut self, n: NodeId, written: Option<&[u8]>) -> Result<(), DomOom> {
        let id = self.intern_written(written)?;
        match self.node(n).map(|x| x.payload) {
            Some(Payload::Element(i)) => {
                if let Some(e) = self.elements.get_mut(i as usize) {
                    e.written = id;
                }
            }
            Some(Payload::Attr(i)) => {
                if let Some(a) = self.attrs.get_mut(i as usize) {
                    a.written = id;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A new element: local name `tag` (an id of [`Document::names`]), in
    /// `ns` (0 none), with `prefix` (0 none), and its qualified name as
    /// written when that differs from the lower-cased local name.
    pub fn create_element(
        &mut self,
        tag: usize,
        ns: NsIndex,
        prefix: PrefixIndex,
        written: Option<&[u8]>,
    ) -> Result<NodeId, DomOom> {
        let written = self.intern_written(written)?;
        let idx = u32::try_from(self.elements.len()).map_err(|_| DomOom)?;
        oom!(self.elements.falloc_reserve(1));
        let id = self.push(NodeType::Element, Payload::Element(idx))?;
        self.set_names(id, tag, ns, prefix);
        self.elements.push(ElementData {
            written,
            first_attr: None,
            last_attr: None,
            id_attr: None,
            class_attr: None,
            template: None,
        });
        /* An HTML `<template>` is made with its contents fragment, as
         * Lexbor's template interface is: in the HTML namespace, hosted by
         * the template. */
        if tag == res::TAG_TEMPLATE && ns == res::NS_HTML as u32 {
            let contents = self.create_fragment(Some(id))?;
            self.set_names(contents, 0, ns, 0);
            self.set_template_content(id, contents);
        }
        Ok(id)
    }

    /// A new attribute, not on any element.
    pub fn create_attr(
        &mut self,
        local: usize,
        ns: NsIndex,
        prefix: PrefixIndex,
        written: Option<&[u8]>,
        value: &[u8],
    ) -> Result<NodeId, DomOom> {
        let written = self.intern_written(written)?;
        let value = self.store(value)?;
        let idx = u32::try_from(self.attrs.len()).map_err(|_| DomOom)?;
        oom!(self.attrs.falloc_reserve(1));
        let id = self.push(NodeType::Attribute, Payload::Attr(idx))?;
        self.set_names(id, local, ns, prefix);
        self.attrs.push(AttrData {
            written,
            value,
            next: None,
            prev: None,
        });
        Ok(id)
    }

    /// A new Text, Comment or CDATA node holding `data`.
    pub fn create_data(&mut self, kind: NodeType, data: &[u8]) -> Result<NodeId, DomOom> {
        debug_assert!(matches!(
            kind,
            NodeType::Text | NodeType::Comment | NodeType::CDataSection
        ));
        let span = self.store(data)?;
        self.push(kind, Payload::Data(span))
    }

    /// A new ProcessingInstruction.
    pub fn create_pi(&mut self, target: &[u8], data: &[u8]) -> Result<NodeId, DomOom> {
        let target = self.store(target)?;
        let data = self.store(data)?;
        self.push(NodeType::Pi, Payload::Pi { target, data })
    }

    /// A new DocumentType.
    pub fn create_doctype(
        &mut self,
        name: &[u8],
        public_id: &[u8],
        system_id: &[u8],
    ) -> Result<NodeId, DomOom> {
        let d = DoctypeData {
            name: self.store(name)?,
            public_id: self.store(public_id)?,
            system_id: self.store(system_id)?,
        };
        let idx = u32::try_from(self.doctypes.len()).map_err(|_| DomOom)?;
        oom!(self.doctypes.falloc_reserve(1));
        let id = self.push(NodeType::DocumentType, Payload::Doctype(idx))?;
        self.doctypes.push(d);
        Ok(id)
    }

    /// A new, empty DocumentFragment, hosted by `host` (a template) or none.
    pub fn create_fragment(&mut self, host: Option<NodeId>) -> Result<NodeId, DomOom> {
        self.push(NodeType::DocumentFragment, Payload::Fragment { host })
    }

    /// Set a node's Lexbor name fields: `local_name`, `ns`, `prefix`.
    pub fn set_names(&mut self, n: NodeId, local: usize, ns: NsIndex, prefix: PrefixIndex) {
        if let Some(node) = self.node_mut(n) {
            node.local = u32::try_from(local).unwrap_or(0);
            node.ns = ns;
            node.prefix = prefix;
        }
    }

    /// Give the template element `el` its contents fragment.
    pub fn set_template_content(&mut self, el: NodeId, contents: NodeId) {
        if let Some(e) = self.element_mut(el) {
            e.template = Some(contents);
        }
    }

    /// Record the start tag's byte offset for `n`.
    pub fn set_source_offset(&mut self, n: NodeId, offset: usize) {
        let v = u32::try_from(offset)
            .ok()
            .and_then(|o| o.checked_add(1))
            .unwrap_or(0);
        if let Some(node) = self.node_mut(n) {
            node.source = v;
        }
    }

    /// Forget `n`'s source offset.
    pub fn forget_source_offset(&mut self, n: NodeId) {
        if let Some(node) = self.node_mut(n) {
            node.source = 0;
        }
    }

    /* ---- linking ---- */

    /// Take `n` out of its parent's child list (`lxb_dom_node_remove`). An
    /// attribute is not in one; see [`Document::attr_remove`].
    pub fn detach(&mut self, n: NodeId) {
        let Some(node) = self.node(n).copied() else {
            return;
        };
        if node.kind == NodeType::Attribute {
            return;
        }
        if let Some(p) = node.parent {
            if let Some(parent) = self.node_mut(p) {
                if parent.first_child == Some(n) {
                    parent.first_child = node.next;
                }
                if parent.last_child == Some(n) {
                    parent.last_child = node.prev;
                }
            }
        }
        if let Some(prev) = node.prev.and_then(|x| self.node_mut(x)) {
            prev.next = node.next;
        }
        if let Some(next) = node.next.and_then(|x| self.node_mut(x)) {
            next.prev = node.prev;
        }
        if let Some(me) = self.node_mut(n) {
            me.parent = None;
            me.next = None;
            me.prev = None;
        }
    }

    /// Append `child` as `parent`'s last child (`lxb_dom_node_insert_child`),
    /// taking it from wherever it was.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        let last = self.node(parent).and_then(|p| p.last_child);
        if let Some(c) = self.node_mut(child) {
            c.parent = Some(parent);
            c.prev = last;
            c.next = None;
        }
        match last.and_then(|l| self.node_mut(l)) {
            Some(l) => l.next = Some(child),
            None => {
                if let Some(p) = self.node_mut(parent) {
                    p.first_child = Some(child);
                }
            }
        }
        if let Some(p) = self.node_mut(parent) {
            p.last_child = Some(child);
        }
    }

    /// Put `node` immediately before `at` (`lxb_dom_node_insert_before`).
    pub fn insert_before(&mut self, at: NodeId, node: NodeId) {
        self.detach(node);
        let Some(target) = self.node(at).copied() else {
            return;
        };
        if let Some(n) = self.node_mut(node) {
            n.parent = target.parent;
            n.prev = target.prev;
            n.next = Some(at);
        }
        match target.prev.and_then(|p| self.node_mut(p)) {
            Some(p) => p.next = Some(node),
            None => {
                if let Some(parent) = target.parent.and_then(|p| self.node_mut(p)) {
                    parent.first_child = Some(node);
                }
            }
        }
        if let Some(t) = self.node_mut(at) {
            t.prev = Some(node);
        }
    }

    /// Put `node` immediately after `at` (`lxb_dom_node_insert_after`).
    pub fn insert_after(&mut self, at: NodeId, node: NodeId) {
        self.detach(node);
        let Some(target) = self.node(at).copied() else {
            return;
        };
        if let Some(n) = self.node_mut(node) {
            n.parent = target.parent;
            n.prev = Some(at);
            n.next = target.next;
        }
        match target.next.and_then(|x| self.node_mut(x)) {
            Some(x) => x.prev = Some(node),
            None => {
                if let Some(parent) = target.parent.and_then(|p| self.node_mut(p)) {
                    parent.last_child = Some(node);
                }
            }
        }
        if let Some(t) = self.node_mut(at) {
            t.next = Some(node);
        }
    }

    /// Append the attribute `attr` to the element `el`'s list, keeping the
    /// `id`/`class` shortcuts as Lexbor's `lxb_dom_element_attr_append`
    /// does for an attribute in no namespace.
    pub fn attr_append(&mut self, el: NodeId, attr: NodeId) {
        if !self.attr_link(el, attr) {
            return;
        }
        let (local, ns) = (self.local(attr), self.own_ns_if_on(attr, el));
        if let Some(e) = self.element_mut(el) {
            if ns == 0 && local == res::ATTR_ID as u32 && e.id_attr.is_none() {
                e.id_attr = Some(attr);
            }
            if ns == 0 && local == res::ATTR_CLASS as u32 && e.class_attr.is_none() {
                e.class_attr = Some(attr);
            }
        }
    }

    /// Link the attribute `attr` last in `el`'s list, leaving the shortcuts
    /// alone; whether both were what they must be.
    pub fn attr_link(&mut self, el: NodeId, attr: NodeId) -> bool {
        let Some(last) = self.element(el).map(|e| e.last_attr) else {
            return false;
        };
        if self.attr(attr).is_none() {
            return false;
        }
        if let Some(a) = self.node_mut(attr) {
            a.parent = Some(el);
        }
        if let Some(a) = self.attr_mut(attr) {
            a.prev = last;
            a.next = None;
        }
        match last.and_then(|l| self.attr_mut(l)) {
            Some(l) => l.next = Some(attr),
            None => {
                if let Some(e) = self.element_mut(el) {
                    e.first_attr = Some(attr);
                }
            }
        }
        if let Some(e) = self.element_mut(el) {
            e.last_attr = Some(attr);
        }
        true
    }

    /// Take `attr` off its element (kept in the arena).
    pub fn attr_remove(&mut self, attr: NodeId) {
        let Some(el) = self.parent(attr) else { return };
        let Some((next, prev)) = self.attr(attr).map(|a| (a.next, a.prev)) else {
            return;
        };
        if let Some(e) = self.element_mut(el) {
            if e.first_attr == Some(attr) {
                e.first_attr = next;
            }
            if e.last_attr == Some(attr) {
                e.last_attr = prev;
            }
            if e.id_attr == Some(attr) {
                e.id_attr = None;
            }
            if e.class_attr == Some(attr) {
                e.class_attr = None;
            }
        }
        if let Some(p) = prev.and_then(|x| self.attr_mut(x)) {
            p.next = next;
        }
        if let Some(n) = next.and_then(|x| self.attr_mut(x)) {
            n.prev = prev;
        }
        if let Some(a) = self.attr_mut(attr) {
            a.next = None;
            a.prev = None;
        }
        if let Some(me) = self.node_mut(attr) {
            me.parent = None;
        }
    }

    /// Point the element's `id`/`class` shortcuts at `attr` (or clear them),
    /// for the DOM attribute algorithms that decide them.
    pub fn set_shortcuts(
        &mut self,
        el: NodeId,
        id_attr: Option<NodeId>,
        class_attr: Option<NodeId>,
    ) {
        if let Some(e) = self.element_mut(el) {
            e.id_attr = id_attr;
            e.class_attr = class_attr;
        }
    }

    /// Replace the data of a Text, Comment, CDATA or PI node, or an
    /// attribute's value. The old bytes stay in the store (append-only).
    pub fn set_data(&mut self, n: NodeId, data: &[u8]) -> Result<(), DomOom> {
        let span = self.store(data)?;
        let Some(node) = self.node(n).copied() else {
            return Ok(());
        };
        match node.payload {
            Payload::Data(_) => {
                if let Some(x) = self.node_mut(n) {
                    x.payload = Payload::Data(span);
                }
            }
            Payload::Pi { target, .. } => {
                if let Some(x) = self.node_mut(n) {
                    x.payload = Payload::Pi { target, data: span };
                }
            }
            Payload::Attr(i) => {
                if let Some(a) = self.attrs.get_mut(i as usize) {
                    a.value = span;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /* ---- reading ---- */

    #[inline]
    pub fn kind(&self, n: NodeId) -> NodeType {
        self.node(n).map_or(NodeType::Other, |x| x.kind)
    }
    /// The tree parent; for an attribute, the element it is on.
    #[inline]
    pub fn parent(&self, n: NodeId) -> Option<NodeId> {
        self.node(n)?.parent
    }
    #[inline]
    pub fn first_child(&self, n: NodeId) -> Option<NodeId> {
        self.node(n)?.first_child
    }
    #[inline]
    pub fn last_child(&self, n: NodeId) -> Option<NodeId> {
        self.node(n)?.last_child
    }
    /// The next sibling; none for an attribute (see
    /// [`next_attr`](Self::next_attr)).
    #[inline]
    pub fn next(&self, n: NodeId) -> Option<NodeId> {
        self.node(n)?.next
    }
    #[inline]
    pub fn prev(&self, n: NodeId) -> Option<NodeId> {
        self.node(n)?.prev
    }
    /// The start tag's byte offset, when recorded.
    #[inline]
    pub fn source_offset(&self, n: NodeId) -> Option<usize> {
        let s = self.node(n)?.source;
        (s != 0).then(|| (s - 1) as usize)
    }

    #[inline]
    pub fn element(&self, n: NodeId) -> Option<&ElementData> {
        match self.node(n)?.payload {
            Payload::Element(i) => self.elements.get(i as usize),
            _ => None,
        }
    }

    #[inline]
    fn element_mut(&mut self, n: NodeId) -> Option<&mut ElementData> {
        match self.node(n)?.payload {
            Payload::Element(i) => self.elements.get_mut(i as usize),
            _ => None,
        }
    }

    #[inline]
    fn attr_mut(&mut self, n: NodeId) -> Option<&mut AttrData> {
        match self.node(n)?.payload {
            Payload::Attr(i) => self.attrs.get_mut(i as usize),
            _ => None,
        }
    }

    /// The next attribute of the same element.
    #[inline]
    pub fn next_attr(&self, n: NodeId) -> Option<NodeId> {
        self.attr(n)?.next
    }
    /// The previous attribute of the same element.
    #[inline]
    pub fn prev_attr(&self, n: NodeId) -> Option<NodeId> {
        self.attr(n)?.prev
    }

    #[inline]
    pub fn attr(&self, n: NodeId) -> Option<&AttrData> {
        match self.node(n)?.payload {
            Payload::Attr(i) => self.attrs.get(i as usize),
            _ => None,
        }
    }

    /// An element's qualified name as written, when it has one of its own.
    pub fn element_written(&self, n: NodeId) -> Option<&[u8]> {
        self.names.qualified(self.element(n)?.written)
    }
    /// [`element_written`](Self::element_written) in ASCII upper case.
    pub fn element_written_upper(&self, n: NodeId) -> Option<&[u8]> {
        self.names.qualified_upper(self.element(n)?.written)
    }
    /// An element's first attribute.
    pub fn first_attr(&self, n: NodeId) -> Option<NodeId> {
        self.element(n)?.first_attr
    }
    /// An element's no-namespace `id` attribute.
    pub fn id_attr(&self, n: NodeId) -> Option<NodeId> {
        self.element(n)?.id_attr
    }
    /// An element's no-namespace `class` attribute.
    pub fn class_attr(&self, n: NodeId) -> Option<NodeId> {
        self.element(n)?.class_attr
    }
    /// A template element's contents fragment.
    pub fn template_content(&self, n: NodeId) -> Option<NodeId> {
        self.element(n)?.template
    }
    /// An attribute's qualified name as written, when it has one of its own.
    pub fn attr_written(&self, n: NodeId) -> Option<&[u8]> {
        self.names.qualified(self.attr(n)?.written)
    }
    /// An attribute's OWN namespace (0 for none), as the adapter's
    /// `HtmlAttr::own_ns` decides it: the stored one when the attribute has a
    /// prefix or differs from its element's, none otherwise.
    pub fn attr_own_ns(&self, n: NodeId) -> NsIndex {
        match self.parent(n) {
            Some(el) => self.own_ns_if_on(n, el),
            None if self.prefix(n) != 0 => self.ns(n),
            None => 0,
        }
    }

    /// [`attr_own_ns`](Self::attr_own_ns) of `attr` as if on `el`.
    fn own_ns_if_on(&self, attr: NodeId, el: NodeId) -> NsIndex {
        let ns = self.ns(attr);
        if self.prefix(attr) != 0 || self.ns(el) != ns {
            ns
        } else {
            0
        }
    }

    /// Lexbor's `local_name` of any node (see the field).
    #[inline]
    pub fn local(&self, n: NodeId) -> u32 {
        self.node(n).map_or(0, |x| x.local)
    }
    /// Lexbor's `ns` of any node, 0 for none.
    #[inline]
    pub fn ns(&self, n: NodeId) -> NsIndex {
        self.node(n).map_or(0, |x| x.ns)
    }
    /// Lexbor's `prefix` of any node, 0 for none.
    #[inline]
    pub fn prefix(&self, n: NodeId) -> PrefixIndex {
        self.node(n).map_or(0, |x| x.prefix)
    }

    /// An attribute's value.
    pub fn attr_value(&self, n: NodeId) -> Option<&[u8]> {
        self.attr(n).map(|a| self.bytes_of(a.value))
    }
    /// The data of a Text, Comment, CDATA or PI node.
    pub fn data(&self, n: NodeId) -> Option<&[u8]> {
        match self.node(n)?.payload {
            Payload::Data(s) | Payload::Pi { data: s, .. } => Some(self.bytes_of(s)),
            _ => None,
        }
    }
    /// A PI's target.
    pub fn pi_target(&self, n: NodeId) -> Option<&[u8]> {
        match self.node(n)?.payload {
            Payload::Pi { target, .. } => Some(self.bytes_of(target)),
            _ => None,
        }
    }
    /// A doctype's name, public id and system id.
    pub fn doctype(&self, n: NodeId) -> Option<(&[u8], &[u8], &[u8])> {
        match self.node(n)?.payload {
            Payload::Doctype(i) => {
                let d = self.doctypes.get(i as usize)?;
                Some((
                    self.bytes_of(d.name),
                    self.bytes_of(d.public_id),
                    self.bytes_of(d.system_id),
                ))
            }
            _ => None,
        }
    }
    /// A fragment's host template.
    pub fn fragment_host(&self, n: NodeId) -> Option<NodeId> {
        match self.node(n)?.payload {
            Payload::Fragment { host } => host,
            _ => None,
        }
    }
}

mod copy;
mod edit;
mod handle;
mod parsed;
/// HTML serialization.
pub mod serialize;
pub use edit::{HtmlDocMut, HtmlTree, Insertion, Place};
pub use handle::{
    has_ascii_uppercase, Ancestors, AttrName, Attrs, HtmlAttr, HtmlDoc, HtmlElement, HtmlNode,
    NsId, RawNode, Siblings, TagId, NS_LAST_ENTRY, TAG_EM_DOCTYPE, TAG_LAST_ENTRY,
};
pub use parsed::{DomIndex, ForeignNode, HtmlDocIdentity, HtmlNodeKey, HtmlParsed, TextRun};

#[cfg(test)]
pub mod conformance;
#[cfg(test)]
mod tests;
