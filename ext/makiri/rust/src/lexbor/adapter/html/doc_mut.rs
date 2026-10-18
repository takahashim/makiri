//! [`HtmlDocMut`]: one document, lent exclusively for an edit.
//!
//! Every change to a document goes through this type - the tree edits, the
//! attribute and data edits, and the factories and copies that add nodes. It
//! is the HTML side's counterpart of the `&mut XmlDoc` the XML bridge lends
//! (`bridge::xml::Editing::with_arena`), and the shape the parser port's arena
//! needs (`docs/rewrite_parser.ja.md`): there an edit holds the arena by `&mut`,
//! so it cannot run while any reader holds a handle into it, and it names nodes
//! by [`RawNode`] - a reference into the document - rather than by handles that
//! borrow it. Reads in the middle of an edit reborrow through [`read`](HtmlDocMut::read).
//!
//! Over Lexbor the type is a thin layer on [`HtmlNodeMut`] and
//! [`HtmlElementMut`], which keep doing the work; what it adds is that a caller
//! outside the adapter cannot reach them without first holding this.

#![allow(unsafe_code)]

use super::*;
use crate::lexbor::adapter::AdapterOom;

/// A document cleared for editing, lent for `'d`.
pub struct HtmlDocMut<'d> {
    doc: HtmlDoc<'d>,
}

impl<'d> HtmlDocMut<'d> {
    /// # Safety
    /// The caller has cleared the document for editing (no XPath evaluation is
    /// reading it, the receiver is not frozen), holds the only edit of it for
    /// `'d`, and runs no Ruby while it is held - so nothing reads a slice or a
    /// handle of the document across a change.
    #[inline]
    pub unsafe fn assume_exclusive(doc: HtmlDoc<'d>) -> Self {
        HtmlDocMut { doc }
    }

    /// The document, for reading between changes.
    #[inline]
    pub fn read(&self) -> HtmlDoc<'_> {
        self.doc
    }

    /// The node `raw` names, for reading between changes.
    ///
    /// Every `RawNode` an edit is given is a node of this document - the
    /// bridge resolves an argument against its own Document and compares
    /// documents before it hands one over.
    #[inline]
    pub fn node(&self, raw: RawNode) -> HtmlNode<'_> {
        // SAFETY: a live node of this document (the type's contract above).
        unsafe { self.doc.node(raw) }
    }

    /// The document as Lexbor names it, for the Lexbor-side facades (the
    /// fragment importer) that take one.
    #[inline]
    pub(in crate::lexbor) fn raw_doc(&self) -> RawDoc {
        RawDoc::from(self.doc)
    }

    /// `raw` as the clearance type that does the work.
    #[inline]
    fn m(&self, raw: RawNode) -> HtmlNodeMut<'_> {
        // SAFETY: this type is the document's clearance for editing.
        unsafe { HtmlNodeMut::assume_mutable(self.node(raw)) }
    }

    /// `raw` as an element cleared for editing; `None` for a non-element.
    #[inline]
    fn el(&self, raw: RawNode) -> Option<HtmlElementMut<'_>> {
        self.m(raw).element_mut()
    }

    /* ---- the tree ---- */

    /// Take `n` out of its tree. The arena keeps it.
    pub fn detach(&mut self, n: RawNode) {
        self.m(n).detach();
    }

    /// Take every child of `n` out of the tree, in order.
    pub fn detach_children(&mut self, n: RawNode) {
        let n = self.m(n);
        while let Some(c) = n.first_child() {
            c.detach();
        }
    }

    /// Put `node` at `place` relative to `target`, after an [`Insertion`] of
    /// the two has checked ([`HtmlNodeMut::place`]).
    pub fn place(&mut self, target: RawNode, node: RawNode, place: Place) {
        self.m(target).place(self.m(node), place);
    }

    /// Take the attribute `attr` off its owner element; a no-op for anything
    /// else ([`HtmlNodeMut::remove_from_owner`]).
    pub fn remove_from_owner(&mut self, attr: RawNode) {
        self.m(attr).remove_from_owner();
    }

    /// DOM `textContent = text` on `n`. `Err` when it could not be stored, in
    /// which case `n` keeps what it had.
    pub fn set_text_content(&mut self, n: RawNode, text: &[u8]) -> Result<(), AdapterOom> {
        self.m(n).set_text_content(text)
    }

    /* ---- attributes ---- */

    /// DOM `setAttribute(name, value)` on the element `el`; a no-op `Ok` for a
    /// non-element (the caller checked).
    pub fn set_attribute(
        &mut self,
        el: RawNode,
        name: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        match self.el(el) {
            Some(e) => e.set_attribute(name, value).map(drop),
            None => Ok(()),
        }
    }

    /// DOM `setAttributeNS(ns, qname, value)` on the element `el`.
    pub fn set_attribute_ns(
        &mut self,
        el: RawNode,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        match self.el(el) {
            Some(e) => e.set_attribute_ns(ns, qname, value),
            None => Ok(()),
        }
    }

    /// Take the attribute `attr` off the element `el`.
    pub fn attr_remove(&mut self, el: RawNode, attr: RawNode) {
        if let (Some(e), Some(a)) = (self.el(el), self.node(attr).attr()) {
            e.attr_remove(a);
        }
    }

    /// DOM `removeAttribute(name)` on the element `el`.
    pub fn remove_attribute(&mut self, el: RawNode, name: &[u8]) {
        if let Some(e) = self.el(el) {
            e.remove_attribute(name);
        }
    }

    /* ---- new nodes ---- */

    /// A new element named `local_name` (DOM `createElement`).
    pub fn create_element(&mut self, local_name: &[u8]) -> Option<RawNode> {
        self.doc.create_element(local_name).map(RawNode::from)
    }

    /// A new element in namespace `ns` with `prefix` (DOM `createElementNS`,
    /// the name already validated and split).
    pub fn create_element_ns(&mut self, local: &[u8], ns: &[u8], prefix: &[u8]) -> Option<RawNode> {
        self.doc
            .create_element_ns(local, ns, prefix)
            .map(RawNode::from)
    }

    /// A new Text node.
    pub fn create_text(&mut self, text: &[u8]) -> Option<RawNode> {
        self.doc.create_text(text).map(RawNode::from)
    }

    /// A new Comment.
    pub fn create_comment(&mut self, text: &[u8]) -> Option<RawNode> {
        self.doc.create_comment(text).map(RawNode::from)
    }

    /// A new ProcessingInstruction.
    pub fn create_pi(&mut self, target: &[u8], data: &[u8]) -> Option<RawNode> {
        self.doc.create_pi(target, data).map(RawNode::from)
    }

    /// A new DocumentType.
    pub fn create_doctype(
        &mut self,
        name: &[u8],
        public_id: Option<&[u8]>,
        system_id: Option<&[u8]>,
    ) -> Option<RawNode> {
        self.doc
            .create_doctype(name, public_id, system_id)
            .map(RawNode::from)
    }

    /// A new, empty DocumentFragment.
    pub fn create_fragment(&mut self) -> Option<RawNode> {
        self.doc.create_fragment().map(RawNode::from)
    }

    /// A detached copy of `src`, `<template>` contents included - from this
    /// document or from another one (DOM `importNode` / `cloneNode`).
    pub fn import(&mut self, src: HtmlNode<'_>, deep: bool) -> Result<RawNode, AdapterOom> {
        // SAFETY: this document is cleared for editing, and `src` is a live
        // handle, so its node is live for the call.
        unsafe {
            crate::lexbor::fragment::import_with_fixup(self.raw_doc(), RawNode::from(src), deep)
        }
    }

    /// [`import`](Self::import) for a source in THIS document, named by
    /// reference: a copy made while holding a read handle into the document
    /// being written is what the arena cannot do.
    pub fn copy(&mut self, src: RawNode, deep: bool) -> Result<RawNode, AdapterOom> {
        // SAFETY: as `import`; `src` is a node of this document.
        unsafe { crate::lexbor::fragment::import_with_fixup(self.raw_doc(), src, deep) }
    }

    /// A detached copy of the XML node `src` of `xdoc`.
    pub fn import_xml(
        &mut self,
        xdoc: &crate::xml::model::Document,
        src: crate::xml::model::NodeId,
        deep: bool,
    ) -> Result<RawNode, crate::xml::model::MutError> {
        // SAFETY: this document is cleared for editing and not restructured
        // by anything else during the call.
        unsafe {
            crate::lexbor::adapter::cross_import::cross_xml_to_html(self.raw_doc(), xdoc, src, deep)
        }
    }
}
