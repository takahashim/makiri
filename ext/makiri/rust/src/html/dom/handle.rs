//! The typed handles over an arena [`Document`], with the API of the Lexbor
//! adapter's (`lexbor::adapter::html`): [`HtmlDoc`], [`HtmlNode`],
//! [`HtmlElement`], [`HtmlAttr`], the ids [`NsId`] and [`TagId`], and the
//! walks. The code above the DOM is written against that API, so it reads
//! either document the same way (`crate::html_dom` chooses which).
//!
//! A handle borrows the document: `&'d Document` and a [`NodeId`]. So a reader
//! and an edit cannot overlap - an edit holds the document by `&mut` - and
//! nothing here needs `unsafe`. Every answer is the one the Lexbor adapter
//! gives for the same tree, read from the fields the arena keeps in Lexbor's
//! shape (`lexbor::conformance_tests` compares the two on parsed documents).

use core::cmp::Ordering;
use core::num::{NonZeroU32, NonZeroUsize};

use super::{Document, NodeId, NodeType};
use crate::html::res::names as res;

/* ---------- ids ---------- */

/// `v` as a non-zero id, for the constants below.
#[allow(
    clippy::panic,
    reason = "only ever evaluated in a const: a zero constant is a build error"
)]
const fn nonzero(v: usize) -> NonZeroUsize {
    match NonZeroUsize::new(v) {
        Some(n) => n,
        None => panic!("an id constant is zero"),
    }
}

#[allow(
    clippy::panic,
    reason = "only ever evaluated in a const: a zero constant is a build error"
)]
const fn nonzero32(v: usize) -> NonZeroU32 {
    match NonZeroU32::new(v as u32) {
        Some(n) => n,
        None => panic!("an id constant is zero"),
    }
}

/// A namespace id of one document - never "none", which is `Option::None`.
/// Lexbor's built-in ids first ([`NsId::is_static`]), then the document's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct NsId(NonZeroU32);

impl NsId {
    pub const HTML: NsId = NsId(nonzero32(res::NS_HTML));
    /// The two foreign roots.
    pub const SVG: NsId = NsId(nonzero32(res::NS_SVG));
    /// See [`NsId::SVG`].
    pub const MATH: NsId = NsId(nonzero32(res::NS_MATH));
    /// The `xml:` namespace.
    pub const XML: NsId = NsId(nonzero32(res::NS_XML));
    /// The namespace of `xmlns` / `xmlns:*` declarations.
    pub const XMLNS: NsId = NsId(nonzero32(res::NS_XMLNS));
    /// The XLink namespace, whose attributes Lexbor writes as `xlink:`.
    pub const XLINK: NsId = NsId(nonzero32(res::NS_XLINK));

    /// The arena's number as an id; `None` for 0.
    #[inline]
    pub(crate) fn from_index(v: u32) -> Option<NsId> {
        NonZeroU32::new(v).map(NsId)
    }

    /// The arena's number for the id (a built-in one is Lexbor's).
    #[inline]
    pub fn number(self) -> u32 {
        self.0.get()
    }

    /// One of Lexbor's built-in namespaces, the same number in every
    /// document.
    #[inline]
    pub fn is_static(self) -> bool {
        (self.0.get() as usize) < NS_LAST_ENTRY
    }
}

/// The end of the built-in namespace ids.
pub const NS_LAST_ENTRY: usize = res::NS__LAST_ENTRY;

/// A tag id: Lexbor's static ids below [`TAG_LAST_ENTRY`], then the
/// document's own. Never `_UNDEF`, which is `Option::None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct TagId(NonZeroUsize);

impl TagId {
    pub const BODY: TagId = TagId(nonzero(res::TAG_BODY));
    pub const SVG: TagId = TagId(nonzero(res::TAG_SVG));
    pub const MATH: TagId = TagId(nonzero(res::TAG_MATH));
    pub const TEMPLATE: TagId = TagId(nonzero(res::TAG_TEMPLATE));
    pub const OPTION: TagId = TagId(nonzero(res::TAG_OPTION));
    pub const OPTGROUP: TagId = TagId(nonzero(res::TAG_OPTGROUP));
    pub const SELECT: TagId = TagId(nonzero(res::TAG_SELECT));
    pub const DATALIST: TagId = TagId(nonzero(res::TAG_DATALIST));
    pub const HR: TagId = TagId(nonzero(res::TAG_HR));

    /// The arena's number as an id; `None` for 0.
    #[inline]
    pub(crate) fn from_index(v: usize) -> Option<TagId> {
        NonZeroUsize::new(v).map(TagId)
    }

    /// The id as an index into the static range, or `None` for a
    /// document's own.
    #[inline]
    pub fn static_index(self) -> Option<usize> {
        let v = self.0.get();
        (v < TAG_LAST_ENTRY).then_some(v)
    }
}

/// The end of the static tag ids.
pub const TAG_LAST_ENTRY: usize = res::TAG__LAST_ENTRY;

/// The last of the special tag ids (text, comment, doctype, document, eof).
pub const TAG_EM_DOCTYPE: usize = res::TAG__EM_DOCTYPE;

/// Whether `name` holds an ASCII upper-case letter - see the adapter's.
#[inline]
pub fn has_ascii_uppercase(name: &[u8]) -> bool {
    name.iter().any(u8::is_ascii_uppercase)
}

/// A DOM `localName` from a qualified name and the stored local name: the
/// qualified name's tail, at the stored name's length, so its case is kept.
fn case_preserved_tail<'a>(qualified: &'a [u8], local: &'a [u8]) -> &'a [u8] {
    if qualified.len() >= local.len() {
        &qualified[qualified.len() - local.len()..]
    } else {
        local
    }
}

/// Whether `qualified` is `name` ASCII-lowercased, without making the copy.
fn eq_lowered(qualified: &[u8], name: &[u8]) -> bool {
    qualified.len() == name.len()
        && qualified
            .iter()
            .zip(name)
            .all(|(&q, &n)| q == n.to_ascii_lowercase())
}

/// An attribute name resolved against one document - see
/// [`HtmlDoc::resolve_attr_name`].
#[derive(Clone, Copy)]
pub struct AttrName {
    /// `None`: not resolvable (a prefixed or empty name) - look up by name.
    doc: Option<*const Document>,
    /// The lower-cased local name's id there; `None`: no attribute of that
    /// document has the name.
    id: Option<u32>,
    /// The name has no ASCII upper case.
    lower: bool,
}

/* ---------- handles ---------- */

/// A document, borrowed for `'d`.
#[derive(Clone, Copy)]
pub struct HtmlDoc<'d> {
    d: &'d Document,
}

impl PartialEq for HtmlDoc<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.d, other.d)
    }
}
impl Eq for HtmlDoc<'_> {}

/// A node of a document, borrowed for `'d`.
#[derive(Clone, Copy)]
pub struct HtmlNode<'d> {
    d: &'d Document,
    id: NodeId,
}

impl PartialEq for HtmlNode<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && core::ptr::eq(self.d, other.d)
    }
}
impl Eq for HtmlNode<'_> {}

/// An element node.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HtmlElement<'d>(HtmlNode<'d>);

/// An attribute node.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HtmlAttr<'d>(HtmlNode<'d>);

/// A node named without a borrow of its document - what crosses the Ruby
/// boundary: its document's stamp and its id, in one word, so two documents'
/// nodes never share one (as two Lexbor nodes never share a pointer). It
/// means something only together with its document ([`HtmlDoc::node`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(transparent)]
pub struct RawNode(core::num::NonZeroU64);

/* `word` is the whole RawNode only where a usize holds 64 bits: on a 32-bit
 * target the stamp would be dropped and two documents' nodes would share a
 * word. The XML arena refuses such targets too (`xml::model::NodeId`). */
#[cfg(not(target_pointer_width = "64"))]
compile_error!(
    "Makiri's HTML index arena packs a document stamp and a NodeId into one \
     pointer-sized word; only 64-bit targets are supported"
);

impl RawNode {
    /// The node `id` of the document stamped `stamp`.
    #[inline]
    pub fn new(stamp: u32, id: NodeId) -> Self {
        RawNode(core::num::NonZeroU64::from(id.0) | (u64::from(stamp) << 32))
    }

    /// The node id, in its document.
    #[inline]
    pub fn id(self) -> NodeId {
        /* The low half is a NodeId's non-zero word, by construction. */
        NodeId(NonZeroU32::new(self.0.get() as u32).unwrap_or(NonZeroU32::MIN))
    }

    /// The stamp of the document the node belongs to.
    #[inline]
    pub fn stamp(self) -> u32 {
        (self.0.get() >> 32) as u32
    }

    /// The node as one word, for the bridge's node word and a token.
    #[inline]
    pub fn word(self) -> usize {
        self.0.get() as usize
    }

    /// A [`word`](Self::word) back; `None` for one no node has.
    #[inline]
    pub fn from_word(w: usize) -> Option<Self> {
        let w = u64::try_from(w).ok()?;
        NonZeroU32::new(w as u32)?;
        core::num::NonZeroU64::new(w).map(RawNode)
    }

    /// The node as a key of a pointer-keyed table - its word, never 0. Not a
    /// pointer to anything: a key is only hashed and compared.
    #[inline]
    pub fn table_key(self) -> *const () {
        core::ptr::without_provenance(self.word())
    }
}

impl crate::ptr_table::TableKey for Option<RawNode> {
    const EMPTY: Self = None;
    #[inline]
    fn table_hash(self) -> u64 {
        crate::ptr_table::ptr_hash(self.map_or(0, RawNode::word) as *const ())
    }
}

impl<'d> From<HtmlNode<'d>> for RawNode {
    #[inline]
    fn from(n: HtmlNode<'d>) -> Self {
        RawNode::new(n.d.stamp(), n.id)
    }
}

impl<'d> HtmlDoc<'d> {
    /// The handle of `d`.
    #[inline]
    pub fn of(d: &'d Document) -> Self {
        HtmlDoc { d }
    }

    /// The arena.
    #[inline]
    pub fn arena(self) -> &'d Document {
        self.d
    }

    /// The id `uri` has in the document's namespace table, without interning
    /// it; byte for byte. `None` for an empty URI or one never interned.
    pub fn lookup_ns(self, uri: &[u8]) -> Option<NsId> {
        self.d.lookup_ns(uri).and_then(NsId::from_index)
    }

    /// Always: every document of the arena is an HTML document.
    #[inline]
    pub fn is_html_document(self) -> bool {
        true
    }

    /// 0 no-quirks, 1 quirks, 2 limited-quirks.
    #[inline]
    pub fn compat_mode(self) -> i64 {
        i64::from(self.d.compat_mode().lexbor())
    }

    /// The tag id `name` is known by (ASCII case-insensitively), or `None`.
    pub fn tag_id(self, name: &[u8]) -> Option<TagId> {
        if name.is_empty() {
            return None;
        }
        self.d.names.tag_id(name).and_then(TagId::from_index)
    }

    /// `name` resolved once for [`HtmlElement::attr_by_resolved_name`] on
    /// this document's elements.
    pub fn resolve_attr_name(self, name: &[u8]) -> AttrName {
        if name.is_empty() || name.contains(&b':') {
            return AttrName {
                doc: None,
                id: None,
                lower: false,
            };
        }
        AttrName {
            doc: Some(self.d as *const Document),
            id: self
                .d
                .names
                .attr_id(name)
                .and_then(|i| u32::try_from(i).ok()),
            lower: !has_ascii_uppercase(name),
        }
    }

    /// The node `raw` names in this document. The bridge resolves a raw
    /// node only against the document it came from; one of another document
    /// is a broken invariant, asserted in a debug build.
    #[inline]
    pub fn node(self, raw: RawNode) -> HtmlNode<'d> {
        debug_assert_eq!(raw.stamp(), self.d.stamp(), "a node of another document");
        HtmlNode {
            d: self.d,
            id: raw.id(),
        }
    }

    /// The node `raw` names, when it is a node of this document.
    #[inline]
    pub fn checked(self, raw: RawNode) -> Option<HtmlNode<'d>> {
        if raw.stamp() != self.d.stamp() {
            return None;
        }
        self.get(raw.id())
    }

    /// The node `id` names, when the document has one by that id.
    #[inline]
    pub fn get(self, id: NodeId) -> Option<HtmlNode<'d>> {
        (id.index() < self.d.node_count()).then_some(HtmlNode { d: self.d, id })
    }

    /// Whether `name` satisfies the DOM's doctype-name production - the
    /// rule the XML factory applies too.
    pub fn valid_doctype_name(name: &[u8]) -> bool {
        crate::xml::dom_name::valid_doctype_name(name)
    }

    /// The document as a node.
    #[inline]
    pub fn as_node(self) -> HtmlNode<'d> {
        HtmlNode {
            d: self.d,
            id: NodeId::DOCUMENT,
        }
    }

    /// The document's title, Lexbor's `lxb_html_document_title`: the first
    /// HTML `<title>` in tree order, its first child if that is a Text node,
    /// with whitespace stripped and collapsed. `None` when there is none - or
    /// when the copy cannot be allocated, as Lexbor's answer is then.
    pub fn title(self) -> Option<std::borrow::Cow<'d, [u8]>> {
        let root = self.as_node();
        let title = root.subtree().skip(1).find(|n| {
            n.node_type() == NodeType::Element
                && n.tag_id() == TagId::from_index(res::TAG_TITLE)
                && n.ns_id() == Some(NsId::HTML)
        })?;
        let text = title.first_child()?;
        if text.node_type() != NodeType::Text {
            return None;
        }
        let data = text.data()?;
        let out = strip_collapse_whitespace(data)?;
        (!out.is_empty()).then_some(out)
    }
}

/// `lexbor_str_strip_collapse_whitespace`, step for step, over a copy -
/// borrowed when it would change nothing.
fn strip_collapse_whitespace(data: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r');
    if data.is_empty() {
        return Some(std::borrow::Cow::Borrowed(data));
    }
    let mut v = crate::falloc::try_to_vec(data)?;
    if ws(v[0]) {
        v[0] = 0x20;
    }
    let (mut offset, mut ws_i) = (0usize, 0usize);
    let len = v.len();
    let mut i = 0;
    while i < len {
        if ws(v[i]) {
            if v[ws_i] != 0x20 {
                v[offset] = 0x20;
                ws_i = offset;
                offset += 1;
            }
        } else {
            if v[ws_i] == 0x20 {
                ws_i = offset;
            }
            v[offset] = v[i];
            offset += 1;
        }
        i += 1;
    }
    if offset != i {
        if offset != 0 && v[offset - 1] == 0x20 {
            offset -= 1;
        }
        v.truncate(offset);
    }
    if v == data {
        return Some(std::borrow::Cow::Borrowed(data));
    }
    Some(std::borrow::Cow::Owned(v))
}

impl<'d> HtmlNode<'d> {
    /// The node's id.
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
    }

    #[inline]
    fn at(self, id: Option<NodeId>) -> Option<Self> {
        id.map(|id| HtmlNode { d: self.d, id })
    }

    #[inline]
    pub fn node_type(self) -> NodeType {
        self.d.kind(self.id)
    }

    /// The parent: for an attribute, the element it is set on.
    #[inline]
    pub fn parent(self) -> Option<Self> {
        self.at(self.d.parent(self.id))
    }
    #[inline]
    pub fn first_child(self) -> Option<Self> {
        self.at(self.d.first_child(self.id))
    }
    #[inline]
    pub fn last_child(self) -> Option<Self> {
        self.at(self.d.last_child(self.id))
    }
    #[inline]
    pub fn next(self) -> Option<Self> {
        self.at(self.d.next(self.id))
    }
    #[inline]
    pub fn prev(self) -> Option<Self> {
        self.at(self.d.prev(self.id))
    }

    /// The children, first to last.
    pub fn children(self) -> Siblings<'d> {
        Siblings(self.first_child())
    }
    /// The ancestors, nearest first.
    pub fn ancestors(self) -> Ancestors<'d> {
        Ancestors(self.parent())
    }

    /// This node and its descendants, in document (pre-)order.
    #[inline]
    pub fn subtree(self) -> impl Iterator<Item = HtmlNode<'d>> {
        core::iter::successors(Some(self), move |n| n.preorder_next(self))
    }

    /// The node after this one in a pre-order walk of `root`'s subtree.
    pub fn preorder_next(self, root: Self) -> Option<Self> {
        self.at(self.d.preorder_next(root.id, self.id))
    }

    /// The node after this one's whole subtree in a pre-order walk of
    /// `root`'s subtree.
    pub fn preorder_next_past(self, root: Self) -> Option<Self> {
        self.at(self.d.preorder_next_past(root.id, self.id))
    }

    /// This node and every node below it, `<template>` contents included.
    #[inline]
    pub fn subtree_with_contents(self) -> impl Iterator<Item = HtmlNode<'d>> {
        core::iter::successors(Some(self), move |n| n.preorder_next_with_contents(self))
    }

    /// [`preorder_next`](Self::preorder_next), entering every HTML
    /// `<template>`'s contents fragment: the template, its contents and their
    /// subtree, then the template's own children.
    pub fn preorder_next_with_contents(self, root: Self) -> Option<Self> {
        if let Some(c) = self.template_content() {
            return Some(c);
        }
        if let Some(c) = self.first_child() {
            return Some(c);
        }
        let mut n = self;
        loop {
            if n == root {
                return None;
            }
            if let Some(s) = n.next() {
                return Some(s);
            }
            n = match n.parent() {
                Some(p) => p,
                None => {
                    let host = n.template_host()?;
                    if let Some(c) = host.first_child() {
                        return Some(c);
                    }
                    host
                }
            };
        }
    }

    /// The `<template>` whose contents fragment this is, or None.
    pub fn template_host(self) -> Option<HtmlNode<'d>> {
        let host = self.fragment_host()?;
        (host.template_content() == Some(self)).then_some(host)
    }

    #[inline]
    pub fn element(self) -> Option<HtmlElement<'d>> {
        (self.node_type() == NodeType::Element).then_some(HtmlElement(self))
    }
    #[inline]
    pub fn attr(self) -> Option<HtmlAttr<'d>> {
        (self.node_type() == NodeType::Attribute).then_some(HtmlAttr(self))
    }

    /// The qualified name: an element's own, any other node's `nodeName`.
    pub fn qualified_name(self) -> &'d [u8] {
        match self.element() {
            Some(el) => el.qualified_name(),
            None => self.node_name(),
        }
    }

    /// DOM `nodeName`, as `lxb_dom_node_name` answers it.
    pub fn node_name(self) -> &'d [u8] {
        match self.node_type() {
            NodeType::Element => self
                .element()
                .and_then(HtmlElement::tag_name)
                .unwrap_or_default(),
            NodeType::Attribute => self
                .attr()
                .map(HtmlAttr::qualified_name)
                .unwrap_or_default(),
            NodeType::Text => b"#text",
            NodeType::CDataSection => b"#cdata-section",
            NodeType::Pi => self.pi_target().unwrap_or_default(),
            NodeType::Comment => b"#comment",
            NodeType::Document => b"#document",
            NodeType::DocumentType => self.d.doctype(self.id).map_or(&[], |(n, _, _)| n),
            NodeType::DocumentFragment => b"#document-fragment",
            _ => &[],
        }
    }

    /// The namespace id, or `None` for none.
    #[inline]
    pub fn ns_id(self) -> Option<NsId> {
        NsId::from_index(self.d.ns(self.id))
    }

    /// The source byte offset the parse recorded, or None.
    #[inline]
    pub fn source_offset(self) -> Option<usize> {
        self.d.source_offset(self.id)
    }

    /// Whether the node's name carries a namespace prefix.
    #[inline]
    pub fn has_prefix(self) -> bool {
        self.d.prefix(self.id) != 0
    }

    /// The interned tag id (Lexbor's `local_name`), or `None`.
    #[inline]
    pub fn tag_id(self) -> Option<TagId> {
        TagId::from_index(self.d.local(self.id) as usize)
    }

    /// The namespace URI, or None.
    pub fn ns_uri(self) -> Option<&'d [u8]> {
        self.d.ns_uri(self.d.ns(self.id))
    }

    /// The document the node belongs to.
    #[inline]
    pub fn owner_document(self) -> HtmlDoc<'d> {
        HtmlDoc { d: self.d }
    }

    /// Whether both nodes belong to the same document.
    #[inline]
    pub fn same_document(self, other: HtmlNode<'_>) -> bool {
        core::ptr::eq(self.d, other.d)
    }

    /// Where `other` falls against `self` in document order, or None when
    /// the two are not ordered (an attribute, different documents, detached
    /// subtrees).
    pub fn document_order(self, other: HtmlNode<'d>) -> Option<Ordering> {
        use Ordering::{Equal, Greater, Less};
        let (a, b) = (self, other);
        if a == b {
            return Some(Equal);
        }
        if a.attr().is_some() || b.attr().is_some() || !a.same_document(b) {
            return None;
        }
        let (da, db) = (a.ancestors().count(), b.ancestors().count());
        let (mut pa, mut pb) = (a, b);
        if da > db {
            for _ in 0..(da - db) {
                pa = pa.parent()?;
            }
            if pa == b {
                return Some(Greater);
            }
        } else if db > da {
            for _ in 0..(db - da) {
                pb = pb.parent()?;
            }
            if pb == a {
                return Some(Less);
            }
        }
        let parent = loop {
            let (qa, qb) = (pa.parent()?, pb.parent()?);
            if qa == qb {
                break qa;
            }
            pa = qa;
            pb = qb;
        };
        let mut c = parent.first_child();
        while let Some(x) = c {
            if x == pa {
                return Some(Less);
            }
            if x == pb {
                return Some(Greater);
            }
            c = x.next();
        }
        None
    }

    /// A processing instruction's target, or None for any other kind.
    pub fn pi_target(self) -> Option<&'d [u8]> {
        self.d.pi_target(self.id)
    }

    /// A doctype's public id, or None for any other kind or an empty id.
    pub fn doctype_public_id(self) -> Option<&'d [u8]> {
        self.d
            .doctype(self.id)
            .map(|(_, p, _)| p)
            .filter(|p| !p.is_empty())
    }
    /// A doctype's system id, or None for any other kind or an empty id.
    pub fn doctype_system_id(self) -> Option<&'d [u8]> {
        self.d
            .doctype(self.id)
            .map(|(_, _, s)| s)
            .filter(|s| !s.is_empty())
    }

    /// A text or CDATA node's data, or None for any other kind.
    pub fn char_data(self) -> Option<&'d [u8]> {
        self.data()
            .filter(|_| matches!(self.node_type(), NodeType::Text | NodeType::CDataSection))
    }

    /// The data of a text, CDATA, comment or processing-instruction node.
    #[inline]
    pub fn data(self) -> Option<&'d [u8]> {
        self.d.data(self.id)
    }

    /// Whether this is an HTML `<template>`.
    pub fn is_html_template(self) -> bool {
        self.node_type() == NodeType::Element
            && self.tag_id() == Some(TagId::TEMPLATE)
            && self.ns_id() == Some(NsId::HTML)
    }

    /// This node's own text: a character-data node's data, or an attribute's
    /// value. None for every other kind.
    pub fn own_text(self) -> Option<&'d [u8]> {
        match self.attr() {
            Some(a) => Some(a.value()),
            None => self.data(),
        }
    }

    /// An HTML `<template>` element's contents fragment, or None.
    pub fn template_content(self) -> Option<HtmlNode<'d>> {
        if !self.is_html_template() {
            return None;
        }
        self.at(self.d.template_content(self.id))
    }

    /// A fragment's host - the `<template>` whose contents it is - or None.
    pub fn fragment_host(self) -> Option<HtmlNode<'d>> {
        self.at(self.d.fragment_host(self.id))
    }

    /// A document node's root element (`documentElement`), or None.
    pub fn document_root(self) -> Option<HtmlNode<'d>> {
        if self.node_type() != NodeType::Document {
            return None;
        }
        self.children().find(|c| c.node_type() == NodeType::Element)
    }
}

impl<'d> HtmlElement<'d> {
    #[inline]
    pub fn node(self) -> HtmlNode<'d> {
        self.0
    }

    #[inline]
    fn d(self) -> &'d Document {
        self.0.d
    }

    /// The qualified name: as written, or the lower-cased local name.
    pub fn qualified_name(self) -> &'d [u8] {
        match self.d().element_written(self.0.id) {
            Some(w) => w,
            None => self.local_name(),
        }
    }
    /// Whether the element records its name as written beside its tag.
    pub fn has_written_name(self) -> bool {
        self.d().element_written(self.0.id).is_some()
    }
    /// The stored local name (the tag's name).
    pub fn local_name(self) -> &'d [u8] {
        self.d()
            .names
            .tag_name(self.d().local(self.0.id) as usize)
            .unwrap_or_default()
    }
    /// The DOM's `localName`, case preserved.
    pub fn dom_local_name(self) -> &'d [u8] {
        case_preserved_tail(self.qualified_name(), self.local_name())
    }
    /// DOM `tagName`: upper case for an HTML element (every document here is
    /// an HTML document), the qualified name otherwise.
    pub fn tag_name(self) -> Option<&'d [u8]> {
        if self.0.ns_id() != Some(NsId::HTML) {
            return Some(self.qualified_name());
        }
        let d = self.d();
        match d.element_written_upper(self.0.id) {
            Some(u) => Some(u),
            None => d.names.tag_upper(d.local(self.0.id) as usize),
        }
    }

    /// The first attribute.
    #[inline]
    pub fn first_attr(self) -> Option<HtmlAttr<'d>> {
        self.0.at(self.d().first_attr(self.0.id)).map(HtmlAttr)
    }

    /// The attributes, in order.
    #[inline]
    pub fn attrs(self) -> Attrs<'d> {
        Attrs(self.first_attr())
    }

    /// Whether this is an HTML element (in an HTML document - every one here).
    fn is_html_in_html_doc(self) -> bool {
        self.0.ns_id() == Some(NsId::HTML)
    }

    /// DOM "get an attribute by name" - see the adapter's.
    pub fn attr_by_name(self, qname: &[u8]) -> Option<HtmlAttr<'d>> {
        if self.is_html_in_html_doc() {
            self.attrs().find(|a| eq_lowered(a.qualified_name(), qname))
        } else {
            self.attrs().find(|a| a.qualified_name() == qname)
        }
    }

    /// [`attr_by_name`](Self::attr_by_name) with `qname` resolved once.
    pub fn attr_by_resolved_name(self, qname: &[u8], name: AttrName) -> Option<HtmlAttr<'d>> {
        let Some(doc) = name.doc else {
            return self.attr_by_name(qname);
        };
        debug_assert!(core::ptr::eq(self.d(), doc));
        let id = name.id?;
        let html = self.is_html_in_html_doc();
        self.attrs().find(|a| {
            a.local_id() == id
                && (a.named_by_local() && (html || name.lower)
                    || if html {
                        eq_lowered(a.qualified_name(), qname)
                    } else {
                        a.qualified_name() == qname
                    })
        })
    }

    /// DOM "get an attribute by namespace and local name".
    pub fn attr_by_ns(self, ns: Option<NsId>, local: &[u8]) -> Option<HtmlAttr<'d>> {
        self.attrs()
            .find(|a| a.own_ns() == ns && a.dom_local_name() == local)
    }

    /// The element's ID attribute (the `attr_id` shortcut).
    #[inline]
    pub fn id_attr(self) -> Option<HtmlAttr<'d>> {
        self.0.at(self.d().id_attr(self.0.id)).map(HtmlAttr)
    }

    /// The element's class attribute (the `attr_class` shortcut).
    #[inline]
    pub fn class_attr(self) -> Option<HtmlAttr<'d>> {
        self.0.at(self.d().class_attr(self.0.id)).map(HtmlAttr)
    }

    /// DOM `hasAttribute(qname)`.
    pub fn has_attribute(self, qname: &[u8]) -> bool {
        self.attr_by_name(qname).is_some()
    }
    /// DOM `getAttribute(qname)`.
    pub fn get_attribute(self, qname: &[u8]) -> Option<&'d [u8]> {
        self.attr_by_name(qname).map(HtmlAttr::value)
    }
}

impl<'d> HtmlAttr<'d> {
    #[inline]
    pub fn node(self) -> HtmlNode<'d> {
        self.0
    }

    #[inline]
    fn d(self) -> &'d Document {
        self.0.d
    }

    /// The next attribute of the same element.
    #[inline]
    pub fn next_attr(self) -> Option<HtmlAttr<'d>> {
        self.0.at(self.d().next_attr(self.0.id)).map(HtmlAttr)
    }

    /// The qualified name: as written, or the lower-cased local name.
    #[inline]
    pub fn qualified_name(self) -> &'d [u8] {
        match self.d().attr_written(self.0.id) {
            Some(w) => w,
            None => self.local_name(),
        }
    }
    /// Whether the qualified name is the lower-cased local name itself.
    #[inline]
    pub(crate) fn named_by_local(self) -> bool {
        self.d().attr_written(self.0.id).is_none()
    }
    /// The id of the lower-cased local name.
    #[inline]
    pub(crate) fn local_id(self) -> u32 {
        self.d().local(self.0.id)
    }
    /// The stored, lower-cased local name.
    #[inline]
    pub fn local_name(self) -> &'d [u8] {
        self.d()
            .names
            .attr_name(self.local_id() as usize)
            .unwrap_or_default()
    }
    /// The DOM's `localName`, case preserved.
    pub fn dom_local_name(self) -> &'d [u8] {
        case_preserved_tail(self.qualified_name(), self.local_name())
    }

    #[inline]
    pub fn value(self) -> &'d [u8] {
        self.d().attr_value(self.0.id).unwrap_or_default()
    }

    /// The attribute's OWN namespace id - see the adapter's `own_ns`.
    pub fn own_ns(self) -> Option<NsId> {
        NsId::from_index(self.d().attr_own_ns(self.0.id))
    }

    /// The attribute's own namespace URI, or None.
    pub fn own_ns_uri(self) -> Option<&'d [u8]> {
        self.own_ns()?;
        self.0.ns_uri()
    }

    /// The element the attribute is set on.
    pub fn owner(self) -> Option<HtmlElement<'d>> {
        self.0.parent().and_then(HtmlNode::element)
    }
}

/// [`HtmlNode::children`].
pub struct Siblings<'d>(Option<HtmlNode<'d>>);

impl<'d> Iterator for Siblings<'d> {
    type Item = HtmlNode<'d>;
    #[inline]
    fn next(&mut self) -> Option<HtmlNode<'d>> {
        let n = self.0?;
        self.0 = n.next();
        Some(n)
    }
}

/// [`HtmlNode::ancestors`].
pub struct Ancestors<'d>(Option<HtmlNode<'d>>);

impl<'d> Iterator for Ancestors<'d> {
    type Item = HtmlNode<'d>;
    #[inline]
    fn next(&mut self) -> Option<HtmlNode<'d>> {
        let n = self.0?;
        self.0 = n.parent();
        Some(n)
    }
}

/// [`HtmlElement::attrs`].
pub struct Attrs<'d>(Option<HtmlAttr<'d>>);

impl<'d> Iterator for Attrs<'d> {
    type Item = HtmlAttr<'d>;
    #[inline]
    fn next(&mut self) -> Option<HtmlAttr<'d>> {
        let a = self.0?;
        self.0 = a.next_attr();
        Some(a)
    }
}
