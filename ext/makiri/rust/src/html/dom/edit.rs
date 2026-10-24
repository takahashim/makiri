//! Editing an arena document: [`HtmlDocMut`], with the Lexbor adapter's
//! method set; [`HtmlNewNodes`], the part of it that only makes detached
//! nodes; and the tree [`crate::dom_rules`] checks an insertion against.
//!
//! Every change reproduces what the adapter does over Lexbor - its factories
//! name a node as Lexbor's constructors do, its attribute writes are the DOM's
//! algorithms as `lexbor::adapter::html::attrs` spells them, and nothing is
//! ever destroyed: a removed node or attribute stays in the arena, where a
//! Ruby wrapper may still name it.

use super::handle::{has_ascii_uppercase, HtmlDoc, HtmlNode, NsId, RawNode};
use super::{Document, DomOom, NodeId, NodeName, NodeType, OwnNs, Shortcut, ShortcutRule, HTML_NS};
use crate::dom_rules::{self, At, PreInsertError, TextContent, Tree};
use crate::falloc::VecGrow;
use crate::html::res::names as res;

/// Where an insertion puts its node, relative to the node it is made on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// As the target's last child.
    Child,
    /// Just before the target.
    Before,
    /// Just after the target.
    After,
    /// In the target's place.
    Replace,
}

/// The arena as [`crate::dom_rules`] reads it. The handles carry their
/// document, so the tree needs none; [`HtmlTree::of`] keeps one for the
/// callers that resolve a recorded node in it.
#[derive(Clone, Copy, Default)]
pub struct HtmlTree<'d> {
    doc: Option<HtmlDoc<'d>>,
}

impl<'d> HtmlTree<'d> {
    pub const fn new() -> Self {
        HtmlTree { doc: None }
    }

    /// The tree of `doc`.
    pub fn of(doc: HtmlDoc<'d>) -> Self {
        HtmlTree { doc: Some(doc) }
    }

    /// The document a recorded node is resolved in, when the tree has one.
    pub fn doc(&self) -> Option<HtmlDoc<'d>> {
        self.doc
    }
}

impl<'d> Tree for HtmlTree<'d> {
    type Node = HtmlNode<'d>;
    #[inline]
    fn node_type(&self, n: HtmlNode<'d>) -> NodeType {
        n.node_type()
    }
    #[inline]
    fn tree_parent(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        match n.node_type() {
            NodeType::Attribute => None,
            _ => n.parent(),
        }
    }
    #[inline]
    fn host(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.fragment_host()
    }
    #[inline]
    fn first_child(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.first_child()
    }
    #[inline]
    fn last_child(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.last_child()
    }
    #[inline]
    fn next_sibling(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.next()
    }
    #[inline]
    fn prev_sibling(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.prev()
    }
}

/// An insertion about to be made - see the adapter's.
#[derive(Clone, Copy)]
pub struct Insertion<'d> {
    parent: HtmlNode<'d>,
    at: At<HtmlNode<'d>>,
    node: HtmlNode<'d>,
    /// The node goes beside, or in place of, itself: no change.
    unchanged: bool,
}

impl<'d> Insertion<'d> {
    /// `node` at `place` relative to `target`.
    pub fn new(
        target: HtmlNode<'d>,
        place: Place,
        node: HtmlNode<'d>,
    ) -> Result<Self, PreInsertError> {
        let parent = || {
            HtmlTree::new()
                .tree_parent(target)
                .ok_or(PreInsertError::NoParent {
                    replacing: place == Place::Replace,
                })
        };
        /* A node beside itself is no change, whether or not it has a
         * parent; in its own place, only once it has one. */
        let beside = matches!(place, Place::Before | Place::After);
        if beside && dom_rules::places_itself(target, node) {
            return Ok(Insertion {
                parent: target,
                at: At::Before(None),
                node,
                unchanged: true,
            });
        }
        let (parent, at) = match place {
            Place::Child => (target, At::Before(None)),
            Place::Before => (parent()?, At::Before(Some(target))),
            Place::After => (parent()?, At::Before(target.next())),
            Place::Replace => (parent()?, At::Replacing(target)),
        };
        Ok(Insertion {
            parent,
            at,
            node,
            unchanged: place == Place::Replace && dom_rules::places_itself(target, node),
        })
    }

    /// Every rule the insertion must keep; none for one that changes nothing.
    pub fn check(&self) -> Result<(), PreInsertError> {
        if self.unchanged {
            return Ok(());
        }
        dom_rules::check(&HtmlTree::new(), self.parent, self.node, self.at)
            .map_err(PreInsertError::Rule)
    }

    /// Whether the insertion puts the node beside, or in place of, itself,
    /// which leaves the tree as it is (`dom_rules::places_itself`): the
    /// caller links nothing.
    pub fn unchanged(&self) -> bool {
        self.unchanged
    }
}

/// Why [`HtmlDocMut::create_pi`] made no ProcessingInstruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatePiError {
    /// The data holds `?>` ([`CreatePiError::MESSAGE`]).
    ClosingInData,
    /// No memory.
    Oom,
}

impl CreatePiError {
    /// What a caller tells a user for [`ClosingInData`](Self::ClosingInData):
    /// the XML side's words for the same rule.
    pub const MESSAGE: &'static str = "processing instruction data must not contain ?>";
}

/// A document lent for an edit.
pub struct HtmlDocMut<'d> {
    d: &'d mut Document,
    /// The document's stamp, for the raw nodes the factories hand out.
    stamp: u32,
}

impl<'d> HtmlDocMut<'d> {
    /// The edit of `d`.
    #[inline]
    pub fn new(d: &'d mut Document) -> Self {
        let stamp = d.stamp();
        HtmlDocMut { d, stamp }
    }

    /// The document, for reading between changes.
    #[inline]
    pub fn read(&self) -> HtmlDoc<'_> {
        HtmlDoc::of(self.d)
    }

    /// The node `raw` names, for reading between changes.
    #[inline]
    pub fn node(&self, raw: RawNode) -> HtmlNode<'_> {
        self.read().node(raw)
    }

    /// The part of this edit that makes detached nodes, for a step that
    /// needs no more (an import into a fragment it made).
    #[inline]
    pub fn new_nodes(&mut self) -> HtmlNewNodes<'_> {
        HtmlNewNodes { d: self.d }
    }

    /// The node `id` of this document, as the handle a caller holds.
    #[inline]
    fn raw(&self, id: NodeId) -> RawNode {
        RawNode::new(self.stamp, id)
    }

    /// The id of `raw`, a node of this document.
    #[inline]
    fn id_of(&self, raw: RawNode) -> NodeId {
        debug_assert_eq!(raw.stamp(), self.stamp, "a node of another document");
        raw.id()
    }

    /// Put every element under `root` (template contents included) that is
    /// in no namespace into `ns`: what a fragment parse whose context
    /// namespace the parser was not given leaves to its import.
    pub fn give_namespace(&mut self, root: RawNode, ns: NsId) -> Result<(), DomOom> {
        let mut todo: Vec<NodeId> = Vec::new();
        for n in self.node(root).subtree_with_contents() {
            if n.node_type() == NodeType::Element && n.ns_id().is_none() {
                todo.falloc_push(n.id()).map_err(|()| DomOom)?;
            }
        }
        for id in todo {
            let (local, prefix) = (self.d.local(id), self.d.prefix(id));
            self.d.set_names(id, local as usize, ns.number(), prefix);
        }
        Ok(())
    }

    /* ---- the tree ---- */

    /// Take `n` out of its tree. The arena keeps it.
    pub fn detach(&mut self, n: RawNode) {
        self.d.detach(self.id_of(n));
    }

    /// Take every child of `n` out of the tree, in order.
    pub fn detach_children(&mut self, n: RawNode) {
        while let Some(c) = self.d.first_child(self.id_of(n)) {
            self.d.detach(c);
        }
    }

    /// Put `node` at `place` relative to `target`, after an [`Insertion`]
    /// of the two has checked. A DocumentFragment contributes its children,
    /// in order, and is left empty; a replace then takes `target` out.
    pub fn place(&mut self, target: RawNode, node: RawNode, place: Place) {
        let (target, node) = (self.id_of(target), self.id_of(node));
        let mut after = target;
        let mut put = |d: &mut Document, c: NodeId| match place {
            Place::Child => d.append_child(target, c),
            Place::Before | Place::Replace => d.insert_before(target, c),
            Place::After => {
                d.insert_after(after, c);
                after = c;
            }
        };
        if self.d.kind(node) == NodeType::DocumentFragment {
            while let Some(c) = self.d.first_child(node) {
                self.d.detach(c);
                put(self.d, c);
            }
        } else {
            put(self.d, node);
        }
        if place == Place::Replace {
            self.d.detach(target);
        }
    }

    /// Take the attribute `attr` off its owner element; a no-op for anything
    /// else.
    pub fn remove_from_owner(&mut self, attr: RawNode) {
        if self.d.kind(self.id_of(attr)) == NodeType::Attribute {
            self.d.attr_remove(self.id_of(attr));
        }
    }

    /// DOM `textContent = text` on `n`. `Err` when it could not be stored,
    /// in which case `n` keeps what it had.
    pub fn set_text_content(&mut self, n: RawNode, text: &[u8]) -> Result<(), DomOom> {
        let n = self.id_of(n);
        match dom_rules::text_content(self.d.kind(n)) {
            TextContent::ReplaceChildren => {
                /* Detached, never destroyed: a wrapper may hold a child. The
                 * text node is made first, so a failure changes nothing. */
                let t = if text.is_empty() {
                    None
                } else {
                    Some(self.new_text(text)?)
                };
                while let Some(c) = self.d.first_child(n) {
                    self.d.detach(c);
                }
                if let Some(t) = t {
                    self.d.append_child(n, t);
                }
                Ok(())
            }
            TextContent::Data | TextContent::AttrValue => self.d.set_data(n, text),
            TextContent::Nothing => Ok(()),
        }
    }

    /* ---- attributes ---- */

    /// DOM `setAttribute(name, value)` on the element `el`; a no-op `Ok` for
    /// a non-element.
    pub fn set_attribute(&mut self, el: RawNode, name: &[u8], value: &[u8]) -> Result<(), DomOom> {
        let Some(e) = self.read().node(el).element() else {
            return Ok(());
        };
        if let Some(at) = e.attr_by_name(name) {
            let at = at.node().id();
            return self.d.set_data(at, value);
        }
        let lower = self.is_html(self.id_of(el));
        let at = self.create_attr(self.id_of(el), None, name, value, lower)?;
        let rule = ShortcutRule::take(self.shortcut_of_new(at, None));
        self.d.link_attribute(self.id_of(el), at, rule);
        Ok(())
    }

    /// DOM `setAttributeNS(ns, qname, value)` on the element `el`, `ns` a URI
    /// or `None` for no namespace.
    pub fn set_attribute_ns(
        &mut self,
        el: RawNode,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), DomOom> {
        let Some(e) = self.read().node(el).element() else {
            return Ok(());
        };
        let local = match qname.iter().position(|&b| b == b':') {
            Some(i) => &qname[i + 1..],
            None => qname,
        };
        let existing = match ns {
            Some(uri) => self
                .read()
                .lookup_ns(uri)
                .and_then(|id| e.attr_by_ns(Some(id), local)),
            None => e.attr_by_ns(None, local),
        };
        if let Some(at) = existing {
            let at = at.node().id();
            return self.d.set_data(at, value);
        }
        self.append_attribute(self.id_of(el), ns, qname, value)
    }

    /// Append a new attribute named `qname` in `ns`, case preserved, without
    /// looking for an existing one - a copy's step.
    pub(super) fn append_attribute(
        &mut self,
        el: NodeId,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), DomOom> {
        let at = self.create_attr(el, ns, qname, value, false)?;
        let rule = ShortcutRule::take(self.shortcut_of_new(at, ns));
        self.d.link_attribute(el, at, rule);
        Ok(())
    }

    /// The shortcut the new attribute `at`, set in `ns`, takes: by its DOM
    /// local name, and none in a namespace - an attribute set IN a namespace
    /// is never the ID or class one.
    fn shortcut_of_new(&self, at: NodeId, ns: Option<&[u8]>) -> Option<Shortcut> {
        if ns.is_some() {
            return None;
        }
        let local = self.read().node(self.raw(at)).attr()?.dom_local_name();
        Shortcut::named(local)
    }

    /// Take the attribute `attr` off the element `el`.
    pub fn attr_remove(&mut self, el: RawNode, attr: RawNode) {
        if self.d.kind(self.id_of(attr)) == NodeType::Attribute
            && self.d.parent(self.id_of(attr)) == Some(self.id_of(el))
        {
            self.d.attr_remove(self.id_of(attr));
        }
    }

    /// DOM `removeAttribute(name)` on the element `el`.
    pub fn remove_attribute(&mut self, el: RawNode, name: &[u8]) {
        let found = self
            .read()
            .node(el)
            .element()
            .and_then(|e| e.attr_by_name(name))
            .map(|a| a.node().id());
        if let Some(a) = found {
            self.d.attr_remove(a);
        }
    }

    fn is_html(&self, el: NodeId) -> bool {
        self.d.ns(el) == HTML_NS
    }

    /// A new attribute of `el`'s document named `qname` in `ns`, not linked,
    /// named as Lexbor's `set_name` / `set_name_ns` name it: the local name
    /// lower-cased; the name as written beside it unless `lower` (a
    /// setAttribute on an HTML element); a namespaced prefixed name split at
    /// its first colon. A no-namespace attribute carries its element's
    /// namespace, Lexbor's spelling of "none".
    fn create_attr(
        &mut self,
        el: NodeId,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
        lower: bool,
    ) -> Result<NodeId, DomOom> {
        /* The glue refuses an empty name, or an empty namespace URI, first
         * (`ArgumentError`); refused here too, as no attribute is made. */
        debug_assert!(!qname.is_empty(), "an empty attribute name");
        if qname.is_empty() {
            return Err(DomOom);
        }
        let d = &mut *self.d;
        let (ns_id, colon) = match ns {
            Some(uri) => (
                d.intern_ns(uri)?.ok_or(DomOom)?,
                qname.iter().position(|&b| b == b':'),
            ),
            None => (d.ns(el), None),
        };
        let (local_name, prefix) = match colon {
            Some(i) => (&qname[i + 1..], d.intern_prefix(&qname[..i])?.unwrap_or(0)),
            None => (qname, 0),
        };
        debug_assert!(!local_name.is_empty(), "an empty attribute local name");
        if local_name.is_empty() {
            return Err(DomOom);
        }
        let local = d
            .names
            .attr_local_name_append(local_name)
            .map_err(|_| DomOom)?;
        let written = (!lower || colon.is_some()).then_some(qname);
        let at = d.create_attr(local, ns_id, prefix, written, value)?;
        /* Made in `ns`, or in none - whatever element it goes on (D1). */
        let own = if ns.is_some() {
            OwnNs::Stored
        } else {
            OwnNs::None
        };
        d.set_attr_own_ns(at, own);
        Ok(at)
    }

    /* ---- new nodes ----
     *
     * The factories answer `None` for out of memory, and for a PI whose data
     * holds `?>`, which Lexbor refused as a failed creation (the glue refuses
     * it first, with an `ArgumentError`). An empty name the glue
     * refuses first with an `ArgumentError`, so here it is a broken
     * invariant - caught in debug builds, refused in release. They keep the
     * Lexbor adapter's `Option`, the shape they were ported in. */

    /// A new element named `local_name` (DOM `createElement`): its tag
    /// lower-cased, in the HTML namespace.
    pub fn create_element(&mut self, local_name: &[u8]) -> Option<RawNode> {
        debug_assert!(!local_name.is_empty(), "an empty element name");
        if local_name.is_empty() {
            return None;
        }
        let tag = self.d.names.tag_append_lower(local_name).ok()?;
        self.d
            .create_element(tag, HTML_NS, 0, None)
            .ok()
            .map(|id| self.raw(id))
    }

    /// A new element in namespace `ns` with `prefix` (DOM `createElementNS`,
    /// the name already validated and split) - see the adapter's: an HTML
    /// name with upper case gets a tag of its own, as written.
    pub fn create_element_ns(&mut self, local: &[u8], ns: &[u8], prefix: &[u8]) -> Option<RawNode> {
        debug_assert!(!local.is_empty(), "an empty element local name");
        if local.is_empty() {
            return None;
        }
        let d = &mut *self.d;
        let upper = has_ascii_uppercase(local);
        let html = d.lookup_ns(ns) == Some(HTML_NS);
        let tag = if upper && html {
            d.names.tag_append_raw(local).ok()?
        } else {
            d.names.tag_append_lower(local).ok()?
        };
        let ns_id = if ns.is_empty() {
            0
        } else {
            d.intern_ns(ns).ok()??
        };
        let prefix_id = if prefix.is_empty() {
            0
        } else {
            d.intern_prefix(prefix).ok()??
        };
        let written = if prefix.is_empty() && !upper {
            None
        } else {
            Some(if prefix.is_empty() {
                crate::falloc::try_to_vec(local)?
            } else {
                crate::falloc::try_concat(&[prefix, b":", local])?
            })
        };
        d.create_element(tag, ns_id, prefix_id, written.as_deref())
            .ok()
            .map(|id| self.raw(id))
    }

    fn new_text(&mut self, text: &[u8]) -> Result<NodeId, DomOom> {
        self.d
            .create_data(NodeType::Text, NodeName::html(res::TAG__TEXT), text)
    }

    /// A new Text node (Lexbor names it `#text`, in the HTML namespace).
    pub fn create_text(&mut self, text: &[u8]) -> Option<RawNode> {
        self.new_text(text).ok().map(|id| self.raw(id))
    }

    /// A new Comment (`!--`, in the HTML namespace).
    pub fn create_comment(&mut self, text: &[u8]) -> Option<RawNode> {
        let name = NodeName::html(res::TAG__EM_COMMENT);
        let c = self.d.create_data(NodeType::Comment, name, text).ok()?;
        Some(self.raw(c))
    }

    /// A new ProcessingInstruction, or why there is none: data holding `?>`,
    /// which would end the instruction where it is written (Lexbor refuses
    /// it too), or no memory. The one place that rule is kept.
    pub fn create_pi(&mut self, target: &[u8], data: &[u8]) -> Result<RawNode, CreatePiError> {
        if data.windows(2).any(|w| w == b"?>") {
            return Err(CreatePiError::ClosingInData);
        }
        self.d
            .create_pi(target, data)
            .map(|id| self.raw(id))
            .map_err(|DomOom| CreatePiError::Oom)
    }

    /// A new DocumentType; an absent id is an empty one.
    pub fn create_doctype(
        &mut self,
        name: &[u8],
        public_id: Option<&[u8]>,
        system_id: Option<&[u8]>,
    ) -> Option<RawNode> {
        self.d
            .create_doctype(
                NodeName::default(),
                name,
                public_id.unwrap_or_default(),
                system_id.unwrap_or_default(),
            )
            .ok()
            .map(|id| self.raw(id))
    }

    /// A new, empty DocumentFragment.
    pub fn create_fragment(&mut self) -> Option<RawNode> {
        self.d.create_fragment(None).ok().map(|id| self.raw(id))
    }

    /// A detached copy of `src`, a node of ANOTHER document, `<template>`
    /// contents included (DOM `importNode`).
    pub fn import(&mut self, src: HtmlNode<'_>, deep: bool) -> Result<RawNode, DomOom> {
        self.d
            .copy_from(src.owner_document().arena(), src.id(), deep)
            .map(|id| self.raw(id))
    }

    /// A detached copy of `src`, a node of this document (`cloneNode`).
    pub fn copy(&mut self, src: RawNode, deep: bool) -> Result<RawNode, DomOom> {
        self.d.copy_within(src.id(), deep).map(|id| self.raw(id))
    }

    /// A detached copy of the XML node `src` of `xdoc`.
    pub fn import_xml(
        &mut self,
        xdoc: &crate::xml::model::Document,
        src: crate::xml::model::NodeId,
        deep: bool,
    ) -> Result<RawNode, crate::xml::model::MutError> {
        super::cross_import::cross_xml_to_html(self, xdoc, src, deep)
    }

    /// The namespace `uri` interned in this document, as written.
    pub fn intern_ns(&mut self, uri: &[u8]) -> Option<NsId> {
        self.d
            .intern_ns(uri)
            .ok()
            .flatten()
            .and_then(NsId::from_index)
    }
}

/// A document lent to make DETACHED nodes - the factories, the copies, and
/// a fragment's import into a fragment just made - while the document's
/// indexes stay ([`HtmlParsed::edit_keeping_indexes`]). That is sound only
/// while nothing in the document's tree changes, so this offers nothing that
/// could change it: no insertion but into a node no tree holds, no attribute
/// or data write.
///
/// [`HtmlParsed::edit_keeping_indexes`]: super::HtmlParsed::edit_keeping_indexes
pub struct HtmlNewNodes<'d> {
    d: &'d mut Document,
}

impl<'d> HtmlNewNodes<'d> {
    #[inline]
    pub(super) fn new(d: &'d mut Document) -> Self {
        HtmlNewNodes { d }
    }

    /// The full edit, for this module's delegation only.
    #[inline]
    fn edit(&mut self) -> HtmlDocMut<'_> {
        HtmlDocMut::new(self.d)
    }

    /// The document, for reading between changes.
    #[inline]
    pub fn read(&self) -> HtmlDoc<'_> {
        HtmlDoc::of(self.d)
    }

    /// Append `node` as the last child of `into` - a node no tree of the
    /// document holds (a fragment this lend made) - or, `node` being a
    /// fragment, its children.
    pub fn append_to_detached(&mut self, into: RawNode, node: RawNode) {
        debug_assert!(
            !self.d.is_connected(into.id()),
            "a node in the document's tree, whose indexes this lend keeps"
        );
        self.edit().place(into, node, Place::Child);
    }

    /// [`HtmlDocMut::give_namespace`], under `root`, a node no tree of the
    /// document holds.
    pub fn give_namespace(&mut self, root: RawNode, ns: NsId) -> Result<(), DomOom> {
        debug_assert!(
            !self.d.is_connected(root.id()),
            "a node in the document's tree, whose indexes this lend keeps"
        );
        self.edit().give_namespace(root, ns)
    }

    /// [`HtmlDocMut::create_element`].
    pub fn create_element(&mut self, local_name: &[u8]) -> Option<RawNode> {
        self.edit().create_element(local_name)
    }

    /// [`HtmlDocMut::create_element_ns`].
    pub fn create_element_ns(&mut self, local: &[u8], ns: &[u8], prefix: &[u8]) -> Option<RawNode> {
        self.edit().create_element_ns(local, ns, prefix)
    }

    /// [`HtmlDocMut::create_text`].
    pub fn create_text(&mut self, text: &[u8]) -> Option<RawNode> {
        self.edit().create_text(text)
    }

    /// [`HtmlDocMut::create_comment`].
    pub fn create_comment(&mut self, text: &[u8]) -> Option<RawNode> {
        self.edit().create_comment(text)
    }

    /// [`HtmlDocMut::create_pi`].
    pub fn create_pi(&mut self, target: &[u8], data: &[u8]) -> Result<RawNode, CreatePiError> {
        self.edit().create_pi(target, data)
    }

    /// [`HtmlDocMut::create_doctype`].
    pub fn create_doctype(
        &mut self,
        name: &[u8],
        public_id: Option<&[u8]>,
        system_id: Option<&[u8]>,
    ) -> Option<RawNode> {
        self.edit().create_doctype(name, public_id, system_id)
    }

    /// [`HtmlDocMut::create_fragment`].
    pub fn create_fragment(&mut self) -> Option<RawNode> {
        self.edit().create_fragment()
    }

    /// [`HtmlDocMut::import`].
    pub fn import(&mut self, src: HtmlNode<'_>, deep: bool) -> Result<RawNode, DomOom> {
        self.edit().import(src, deep)
    }

    /// [`HtmlDocMut::copy`].
    pub fn copy(&mut self, src: RawNode, deep: bool) -> Result<RawNode, DomOom> {
        self.edit().copy(src, deep)
    }

    /// [`HtmlDocMut::import_xml`].
    pub fn import_xml(
        &mut self,
        xdoc: &crate::xml::model::Document,
        src: crate::xml::model::NodeId,
        deep: bool,
    ) -> Result<RawNode, crate::xml::model::MutError> {
        self.edit().import_xml(xdoc, src, deep)
    }

    /// [`HtmlDocMut::intern_ns`].
    pub fn intern_ns(&mut self, uri: &[u8]) -> Option<NsId> {
        self.edit().intern_ns(uri)
    }
}
