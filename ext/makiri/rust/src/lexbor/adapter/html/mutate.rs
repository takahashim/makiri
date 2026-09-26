//! Editing a tree: the clearance types [`HtmlNodeMut`] and [`HtmlElementMut`],
//! and the rules an [`Insertion`] must keep before it is [placed](HtmlNodeMut::place).

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use super::*;
use crate::dom_rules::{self, At, Hierarchy, Tree, Violation};

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

/// Why an insertion is refused: a place with no parent to resolve to, or one
/// of the DOM's own rules ([`crate::dom_rules`]). Checked before any link
/// changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreInsertError {
    /// A sibling place, or a replace, on a node with no parent.
    NoParent,
    /// A rule of the WHATWG DOM's "ensure pre-insertion validity".
    Rule(Violation),
}

/// The Lexbor tree as [`crate::dom_rules`] reads it: the nodes carry their
/// own links, so there is nothing to hold but the lifetime.
#[derive(Clone, Copy, Default)]
pub struct HtmlTree<'d>(core::marker::PhantomData<HtmlNode<'d>>);

impl<'d> Tree for HtmlTree<'d> {
    type Node = HtmlNode<'d>;
    #[inline]
    fn node_type(&self, n: HtmlNode<'d>) -> NodeType {
        n.node_type()
    }
    /// Not [`HtmlNode::parent`], which answers an attribute's owner element:
    /// an attribute has no parent in the tree.
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
    fn next_sibling(&self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.next()
    }
}

/// An insertion about to be made: `node` at `place` relative to `target`,
/// resolved to the parent it goes under and what it does at the reference
/// child (goes before it - none for an append - or replaces it).
///
/// A value rather than loose arguments because the positions are easy to
/// swap and mean different things, and the checks read them as a unit.
#[derive(Clone, Copy)]
pub struct Insertion<'d> {
    target: HtmlNode<'d>,
    parent: HtmlNode<'d>,
    at: At<HtmlNode<'d>>,
    node: HtmlNode<'d>,
}

impl<'d> Insertion<'d> {
    /// `node` at `place` relative to `target`; [`PreInsertError::NoParent`]
    /// for a place that needs `target`'s parent when it has none - and an
    /// attribute's owner element is not one.
    pub fn new(
        target: HtmlNode<'d>,
        place: Place,
        node: HtmlNode<'d>,
    ) -> Result<Self, PreInsertError> {
        let parent = || {
            HtmlTree::default()
                .tree_parent(target)
                .ok_or(PreInsertError::NoParent)
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

    /// Every rule the insertion must keep ([`dom_rules::check`]), plus one the
    /// placing needs: a node is not put beside, or in place of, itself -
    /// [`HtmlNodeMut::place`] would detach it from the very position it is
    /// placed at. That one is reported as the own-subtree rule, as it always
    /// was.
    pub fn check(&self) -> Result<(), PreInsertError> {
        if self.target == self.node {
            return Err(PreInsertError::Rule(Violation::HierarchyRequest(
                Hierarchy::Ancestor,
            )));
        }
        dom_rules::check(&HtmlTree::default(), self.parent, self.node, self.at)
            .map_err(PreInsertError::Rule)
    }
}

/// A node the caller has cleared for editing.
///
/// Editing a tree is not something any handle should be able to do: a frozen
/// receiver must refuse, and a document an XPath handler is evaluating over must
/// refuse too, because the engine borrows names and index slices across the walk
/// (see `bridge::wrapper::DocumentEvaluation`). Those two checks live in one place,
/// `bridge::html::edit`, and this type is what that place
/// hands back - so a node that has not been through them has no edit to call.
///
/// Reading needs no such clearance, so [`node`](Self::node) goes back down to
/// the ordinary handle, and the links below carry the clearance to a node of the
/// same document, which the same two checks covered.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlNodeMut<'doc>(HtmlNode<'doc>);

impl<'doc> HtmlNodeMut<'doc> {
    /// # Safety
    /// The caller has established that `node`'s document may be changed now:
    /// the receiver is not frozen, and no XPath evaluation is reading it.
    #[inline]
    pub unsafe fn assume_mutable(node: HtmlNode<'doc>) -> Self {
        HtmlNodeMut(node)
    }

    /// The node as an ordinary handle, for reading.
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }

    /// An HTML `<template>`'s separate contents fragment, as a mutable handle -
    /// what `template.innerHTML = ...` (WHATWG) targets. `None` for a
    /// non-template, or a template Lexbor gave no contents fragment.
    #[inline]
    pub fn template_content_mut(self) -> Option<HtmlNodeMut<'doc>> {
        self.0.template_content().map(HtmlNodeMut)
    }

    #[inline]
    pub(in crate::lexbor::adapter) fn as_raw(self) -> *mut LxbNode {
        self.0.as_raw()
    }

    /* The links: same document, so the caller's clearance covers them too. */

    #[inline]
    pub fn parent(self) -> Option<Self> {
        self.0.parent().map(HtmlNodeMut)
    }

    #[inline]
    pub fn first_child(self) -> Option<Self> {
        self.0.first_child().map(HtmlNodeMut)
    }

    #[inline]
    pub fn next(self) -> Option<Self> {
        self.0.next().map(HtmlNodeMut)
    }

    /// Replace the node's descendants with `text`, DOM `textContent=`.
    ///
    /// `Err` when Lexbor could not store it, in which case the node keeps
    /// what it had.
    pub fn set_text_content(self, text: &[u8]) -> Result<(), AdapterOom> {
        let node = self.node();
        if matches!(
            node.node_type(),
            NodeType::Element | NodeType::DocumentFragment
        ) {
            /* Not Lexbor's own `text_content_set` here: for a container it
             * DESTROYS the old children (`lxb_dom_node_replace_all` ->
             * `destroy_deep`), and a Ruby wrapper may still hold any of them.
             * The memory went back to the arena, the next node allocated there
             * came back under the old wrapper - a text node answering as an
             * Element or an Attr - and the old wrapper read freed memory.
             * Makiri detaches, never destroys: the same result (a non-empty
             * text node made first, so a failure changes nothing), with the old
             * children kept for their wrappers. Empty content creates no Text. */
            let text_node = if text.is_empty() {
                None
            } else {
                Some(node.owner_document().create_text(text).ok_or(AdapterOom)?)
            };
            while let Some(c) = self.first_child() {
                c.detach();
            }
            if let Some(text_node) = text_node {
                /* A detached node of the document `self` may change, just made:
                 * as changeable as `self`. */
                self.insert_child(HtmlNodeMut(text_node.node()));
            }
            return Ok(());
        }
        // SAFETY: a live node the caller may change; for a character-data node
        // (or an attribute) Lexbor replaces the bytes in place and frees no node.
        lexbor_ok(unsafe {
            lxb::lxb_dom_node_text_content_set(self.as_raw(), text.as_ptr(), text.len())
        })
    }

    /// The node as an element cleared for editing, when it is one.
    #[inline]
    pub fn element_mut(self) -> Option<HtmlElementMut<'doc>> {
        self.0.element().map(HtmlElementMut)
    }

    /// Take the node out of its tree. The arena keeps it, so a Ruby wrapper
    /// that still points at it stays valid - Makiri detaches, never destroys.
    #[inline]
    pub fn detach(self) {
        // SAFETY: a live node of a document the caller may change.
        unsafe { lxb::lxb_dom_node_remove(self.as_raw()) };
    }

    /// Append `node` as the last child of `self`.
    #[inline]
    pub fn insert_child(self, node: HtmlNodeMut<'doc>) {
        // SAFETY: two live nodes of a document the caller may change.
        unsafe { lxb::lxb_dom_node_insert_child(self.as_raw(), node.as_raw()) };
    }

    /// Put `node` immediately before `self`.
    #[inline]
    pub fn insert_before(self, node: HtmlNodeMut<'doc>) {
        // SAFETY: as above.
        unsafe { lxb::lxb_dom_node_insert_before(self.as_raw(), node.as_raw()) };
    }

    /// Put `node` immediately after `self`.
    #[inline]
    pub fn insert_after(self, node: HtmlNodeMut<'doc>) {
        // SAFETY: as above.
        unsafe { lxb::lxb_dom_node_insert_after(self.as_raw(), node.as_raw()) };
    }

    /// Put `node` at `place` relative to `self`, after an [`Insertion`] of the
    /// two has checked. A DOCUMENT_FRAGMENT contributes its CHILDREN, in order,
    /// and is left empty, as the DOM's insertion does; a replace then takes
    /// `self` out.
    pub fn place(self, node: HtmlNodeMut<'doc>, place: Place) {
        let mut after = self;
        let mut put = |c: HtmlNodeMut<'doc>| match place {
            Place::Child => self.insert_child(c),
            Place::Before | Place::Replace => self.insert_before(c),
            Place::After => {
                after.insert_after(c);
                after = c; /* the next one goes after this one */
            }
        };
        if node.node().node_type() == NodeType::DocumentFragment {
            while let Some(c) = node.first_child() {
                c.detach();
                put(c);
            }
        } else {
            put(node);
        }
        if place == Place::Replace {
            self.detach();
        }
    }
}

/// An element cleared for editing, reached through [`HtmlNodeMut::element_mut`].
///
/// Same clearance as the node it came from: the receiver was not frozen, and no
/// XPath evaluation is reading its document.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HtmlElementMut<'doc>(HtmlElement<'doc>);

impl<'doc> HtmlElementMut<'doc> {
    /// The element as an ordinary handle, for reading.
    #[inline]
    pub fn element(self) -> HtmlElement<'doc> {
        self.0
    }

    /* The DOM's attribute algorithms (`attrs`), for a receiver cleared for
     * editing. None of them destroys an attribute: a Ruby wrapper may hold
     * one that is replaced or removed. */

    /// DOM `setAttribute(name, value)`. `Err` when Lexbor could not store it.
    pub fn set_attribute(self, name: &[u8], value: &[u8]) -> Result<HtmlAttr<'doc>, AdapterOom> {
        // SAFETY: this type's clearance - the element may be changed.
        unsafe { self.0.set_attribute_value(name, value) }
    }

    /// DOM `setAttributeNS(ns, qname, value)`, `ns` a URI or `None` for no
    /// namespace. `Err` when Lexbor could not store it.
    pub fn set_attribute_ns(
        self,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        // SAFETY: as above.
        unsafe { self.0.set_attribute_value_ns(ns, qname, value) }
    }

    /// Take `attr` off the element. The arena keeps it, like a detached node.
    pub fn attr_remove(self, attr: HtmlAttr<'doc>) {
        if attr.owner() == Some(self.0) {
            // SAFETY: as above, and `attr` is this element's.
            unsafe { self.0.unlink_attr(attr) };
        }
    }

    /// DOM `removeAttribute(name)`: take off the attribute
    /// [`attr_by_name`](HtmlElement::attr_by_name) finds; no-op when there is
    /// none. Detached, as [`attr_remove`](Self::attr_remove) does - never
    /// `lxb_dom_element_remove_attribute`, which DESTROYS it while a Ruby
    /// wrapper may still hold it.
    pub fn remove_attribute(self, name: &[u8]) {
        if let Some(attr) = self.0.attr_by_name(name) {
            self.attr_remove(attr);
        }
    }
}
