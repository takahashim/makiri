//! Editing an arena document: [`HtmlDocMut`], with the Lexbor adapter's
//! method set, and the tree [`crate::dom_rules`] checks an insertion against.
//!
//! Every change reproduces what the adapter does over Lexbor - its factories
//! name a node as Lexbor's constructors do, its attribute writes are the DOM's
//! algorithms as `lexbor::adapter::html::attrs` spells them, and nothing is
//! ever destroyed: a removed node or attribute stays in the arena, where a
//! Ruby wrapper may still name it.

use super::handle::{has_ascii_uppercase, HtmlDoc, HtmlNode, NsId, RawNode};
use super::{Document, DomOom, NodeId, NodeType};
use crate::dom_rules::{self, At, Hierarchy, PreInsertError, Tree, Violation};
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
    target: HtmlNode<'d>,
    parent: HtmlNode<'d>,
    at: At<HtmlNode<'d>>,
    node: HtmlNode<'d>,
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
        let (parent, at) = match place {
            Place::Child => (target, At::Before(None)),
            Place::Before => (parent()?, At::Before(Some(target))),
            Place::After => (parent()?, At::Before(target.next())),
            Place::Replace => (parent()?, At::Replacing(target)),
        };
        Ok(Insertion {
            target,
            parent,
            at,
            node,
        })
    }

    /// Every rule the insertion must keep, plus: a node is not put beside,
    /// or in place of, itself.
    pub fn check(&self) -> Result<(), PreInsertError> {
        if self.target == self.node {
            return Err(PreInsertError::Rule(Violation::HierarchyRequest(
                Hierarchy::Ancestor,
            )));
        }
        dom_rules::check(&HtmlTree::new(), self.parent, self.node, self.at)
            .map_err(PreInsertError::Rule)
    }
}

/// Which attribute shortcut a no-namespace attribute with this local name is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Id,
    Class,
}

impl Shortcut {
    fn named(local: &[u8]) -> Option<Shortcut> {
        match local {
            b"id" => Some(Shortcut::Id),
            b"class" => Some(Shortcut::Class),
            _ => None,
        }
    }
}

/// A document lent for an edit.
pub struct HtmlDocMut<'d> {
    d: &'d mut Document,
}

impl<'d> HtmlDocMut<'d> {
    /// The edit of `d`.
    #[inline]
    pub fn new(d: &'d mut Document) -> Self {
        HtmlDocMut { d }
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

    /// The arena, for the parts of the crate that build into it.
    #[inline]
    pub fn arena(&mut self) -> &mut Document {
        self.d
    }

    /* ---- the tree ---- */

    /// Take `n` out of its tree. The arena keeps it.
    pub fn detach(&mut self, n: RawNode) {
        self.d.detach(n.id());
    }

    /// Take every child of `n` out of the tree, in order.
    pub fn detach_children(&mut self, n: RawNode) {
        while let Some(c) = self.d.first_child(n.id()) {
            self.d.detach(c);
        }
    }

    /// Put `node` at `place` relative to `target`, after an [`Insertion`]
    /// of the two has checked. A DocumentFragment contributes its children,
    /// in order, and is left empty; a replace then takes `target` out.
    pub fn place(&mut self, target: RawNode, node: RawNode, place: Place) {
        let (target, node) = (target.id(), node.id());
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
        if self.d.kind(attr.id()) == NodeType::Attribute {
            self.d.attr_remove(attr.id());
        }
    }

    /// DOM `textContent = text` on `n`. `Err` when it could not be stored,
    /// in which case `n` keeps what it had.
    pub fn set_text_content(&mut self, n: RawNode, text: &[u8]) -> Result<(), DomOom> {
        let n = n.id();
        match self.d.kind(n) {
            NodeType::Element | NodeType::DocumentFragment => {
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
            /* Lexbor's `text_content_set`: an attribute's value, or the data
             * of a text, comment or PI; a CDATA section, a doctype and a
             * document are left as they are. */
            NodeType::Attribute | NodeType::Text | NodeType::Pi | NodeType::Comment => {
                self.d.set_data(n, text)
            }
            _ => Ok(()),
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
        let lower = self.is_html(el.id());
        let at = self.create_attr(el.id(), None, name, value, lower)?;
        let local = self
            .read()
            .node(RawNode::from_id(at))
            .attr()
            .map(|a| a.dom_local_name());
        let shortcut = local.and_then(Shortcut::named);
        self.link_attr(el.id(), at, shortcut);
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
        self.append_attribute(el.id(), ns, qname, value)
    }

    /// Append a new attribute named `qname` in `ns`, case preserved, without
    /// looking for an existing one - a copy's step.
    pub fn append_attribute(
        &mut self,
        el: NodeId,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), DomOom> {
        let at = self.create_attr(el, ns, qname, value, false)?;
        /* An attribute set IN a namespace is never the ID or class one. */
        let shortcut = if ns.is_none() {
            self.read()
                .node(RawNode::from_id(at))
                .attr()
                .and_then(|a| Shortcut::named(a.dom_local_name()))
        } else {
            None
        };
        self.link_attr(el, at, shortcut);
        Ok(())
    }

    /// Take the attribute `attr` off the element `el`.
    pub fn attr_remove(&mut self, el: RawNode, attr: RawNode) {
        if self.d.kind(attr.id()) == NodeType::Attribute
            && self.d.parent(attr.id()) == Some(el.id())
        {
            self.d.attr_remove(attr.id());
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
        self.d.ns(el) == res::NS_HTML as u32
    }

    /// Lexbor's `link_attr`: link `at` last, leaving both shortcuts as they
    /// were, then point `shortcut` (if any) at it.
    fn link_attr(&mut self, el: NodeId, at: NodeId, shortcut: Option<Shortcut>) {
        let (id, class) = (self.d.id_attr(el), self.d.class_attr(el));
        self.d.attr_link(el, at);
        let (id, class) = match shortcut {
            Some(Shortcut::Id) => (Some(at), class),
            Some(Shortcut::Class) => (id, Some(at)),
            None => (id, class),
        };
        self.d.set_shortcuts(el, id, class);
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
        if local_name.is_empty() {
            return Err(DomOom);
        }
        let local = d
            .names
            .attr_local_name_append(local_name)
            .map_err(|_| DomOom)?;
        let written = (!lower || colon.is_some()).then_some(qname);
        d.create_attr(local, ns_id, prefix, written, value)
    }

    /* ---- new nodes ---- */

    /// A new element named `local_name` (DOM `createElement`): its tag
    /// lower-cased, in the HTML namespace.
    pub fn create_element(&mut self, local_name: &[u8]) -> Option<RawNode> {
        if local_name.is_empty() {
            return None;
        }
        let tag = self.d.names.tag_append_lower(local_name).ok()?;
        self.d
            .create_element(tag, res::NS_HTML as u32, 0, None)
            .ok()
            .map(RawNode::from_id)
    }

    /// A new element in namespace `ns` with `prefix` (DOM `createElementNS`,
    /// the name already validated and split) - see the adapter's: an HTML
    /// name with upper case gets a tag of its own, as written.
    pub fn create_element_ns(&mut self, local: &[u8], ns: &[u8], prefix: &[u8]) -> Option<RawNode> {
        if local.is_empty() {
            return None;
        }
        let d = &mut *self.d;
        let upper = has_ascii_uppercase(local);
        let html = d.lookup_ns(ns) == Some(res::NS_HTML as u32);
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
            let mut w: Vec<u8> =
                crate::falloc::try_vec_with_capacity(prefix.len() + 1 + local.len())?;
            if !prefix.is_empty() {
                w.extend_from_slice(prefix);
                w.push(b':');
            }
            w.extend_from_slice(local);
            Some(w)
        };
        d.create_element(tag, ns_id, prefix_id, written.as_deref())
            .ok()
            .map(RawNode::from_id)
    }

    fn new_text(&mut self, text: &[u8]) -> Result<NodeId, DomOom> {
        let t = self.d.create_data(NodeType::Text, text)?;
        self.d.set_names(t, res::TAG__TEXT, res::NS_HTML as u32, 0);
        Ok(t)
    }

    /// A new Text node (Lexbor names it `#text`, in the HTML namespace).
    pub fn create_text(&mut self, text: &[u8]) -> Option<RawNode> {
        self.new_text(text).ok().map(RawNode::from_id)
    }

    /// A new Comment (`!--`, in the HTML namespace).
    pub fn create_comment(&mut self, text: &[u8]) -> Option<RawNode> {
        let c = self.d.create_data(NodeType::Comment, text).ok()?;
        self.d
            .set_names(c, res::TAG__EM_COMMENT, res::NS_HTML as u32, 0);
        Some(RawNode::from_id(c))
    }

    /// A new ProcessingInstruction; `None` for data holding `?>`, which
    /// Lexbor refuses.
    pub fn create_pi(&mut self, target: &[u8], data: &[u8]) -> Option<RawNode> {
        if data.windows(2).any(|w| w == b"?>") {
            return None;
        }
        self.d.create_pi(target, data).ok().map(RawNode::from_id)
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
                name,
                public_id.unwrap_or_default(),
                system_id.unwrap_or_default(),
            )
            .ok()
            .map(RawNode::from_id)
    }

    /// A new, empty DocumentFragment.
    pub fn create_fragment(&mut self) -> Option<RawNode> {
        self.d.create_fragment(None).ok().map(RawNode::from_id)
    }

    /// A detached copy of `src`, a node of ANOTHER document, `<template>`
    /// contents included (DOM `importNode`).
    pub fn import(&mut self, src: HtmlNode<'_>, deep: bool) -> Result<RawNode, DomOom> {
        self.d
            .copy_from(src.owner_document().arena(), src.id(), deep)
            .map(RawNode::from_id)
    }

    /// A detached copy of `src`, a node of this document (`cloneNode`).
    pub fn copy(&mut self, src: RawNode, deep: bool) -> Result<RawNode, DomOom> {
        self.d.copy_within(src.id(), deep).map(RawNode::from_id)
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
