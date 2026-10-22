//! Making nodes: the document's factories, and the handles for a node that is
//! being built and not yet in a tree ([`BuildingNode`], [`BuildingElement`]).

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use super::*;

impl<'doc> HtmlDoc<'doc> {
    /// Whether `name` satisfies the DOM's doctype-name production, which
    /// [`create_doctype`](Self::create_doctype) requires of its caller.
    ///
    /// [`crate::xml::dom_name::valid_doctype_name`], the rule the XML factory
    /// applies too, so the two answer alike from one definition. It is byte
    /// for byte `lxb_dom_document_type_valid_name` - whitespace, NUL and `>`
    /// refused, and the empty name, which Lexbor reads as absent - which this
    /// once called through FFI (and which Lexbor's create still applies).
    pub fn valid_doctype_name(name: &[u8]) -> bool {
        crate::xml::dom_name::valid_doctype_name(name)
    }

    /// An empty detached DocumentFragment. `None` on allocation failure.
    pub(in crate::lexbor) fn create_fragment(self) -> Option<BuildingNode<'doc>> {
        // SAFETY: a live document.
        unsafe {
            BuildingNode::from_raw(
                lxb::lxb_dom_document_create_document_fragment(self.as_raw()) as *mut LxbNode,
            )
        }
    }

    /// Copy `src` into this document, DOM `importNode`. `None` when Lexbor
    /// could not.
    ///
    /// The copy is detached and belongs here, which is what [`BuildingNode`]
    /// says. `deep` carries the subtree - but NOT a `<template>`'s separate
    /// contents fragment, which Lexbor's importNode omits; the caller fixes
    /// that up (see `lexbor::fragment::import_with_fixup`).
    ///
    /// Lexbor's copy appends attributes by its own rules, which can drop one;
    /// `attrs::repair_import` puts the copy's attributes right before it is
    /// handed out, and a copy it cannot repair is `None` too.
    ///
    /// An attribute and a document fragment are copied by steps of their
    /// own (hardening): an attribute by Lexbor's attribute clone, a fragment
    /// made afresh and given copies of the children. Lexbor's importNode is
    /// given every other kind.
    pub(in crate::lexbor) fn import_node(
        self,
        src: HtmlNode<'_>,
        deep: bool,
    ) -> Option<BuildingNode<'doc>> {
        match src.node_type() {
            NodeType::Attribute => {
                // SAFETY: a live attribute, only read; the clone is made in
                // this document, unlinked and owned by no element.
                let copy = unsafe {
                    lxb::lxb_dom_attr_interface_clone(self.as_raw(), src.as_raw() as *mut LxbAttr)
                };
                // SAFETY: Lexbor's attribute begins with its node.
                let copy = unsafe { BuildingNode::from_raw(copy as *mut LxbNode) }?;
                /* The clone re-interns a namespace from another document
                 * case-folded, as an element's copy does: give it the
                 * source's as written. */
                if src.owner_document() != self {
                    // SAFETY: the clone is unlinked and nothing else holds it.
                    unsafe { super::attrs::restore_ns(self, src, copy.0) }.ok()?;
                }
                return Some(copy);
            }
            NodeType::DocumentFragment => {
                let copy = self.create_fragment()?;
                if deep {
                    /* A child is never a fragment, so this goes one level. */
                    for child in src.children() {
                        copy.insert_child(self.import_node(child, true)?);
                    }
                }
                return Some(copy);
            }
            _ => {}
        }
        // SAFETY: two live documents' nodes; Lexbor allocates the copy in this
        // one and leaves the source alone. The copy is unshared until
        // returned, which is what the repair asks.
        unsafe {
            let copy = BuildingNode::from_raw(lxb::lxb_dom_document_import_node(
                self.as_raw(),
                src.as_raw(),
                deep,
            ))?;
            super::attrs::repair_import(self, src, copy.0, deep).ok()?;
            Some(copy)
        }
    }
}

/// A node being built, before anything links it into the destination tree.
///
/// Cross-document translation makes one node at a time and inserts it under a
/// node it made a moment earlier. Neither the frozen check nor the evaluation
/// check behind [`HtmlNodeMut`] applies, because no wrapper has seen these
/// nodes and no query can reach them yet.
///
/// Neither this type nor [`BuildingElement`] has a `Drop`, and that is the
/// contract rather than an omission: a translation that fails part-way leaves
/// the half-built subtree for the document's arena to reclaim wholesale, and a
/// `Drop` here would instead destroy nodes already linked under their parent.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BuildingNode<'doc>(HtmlNode<'doc>);

impl<'doc> BuildingNode<'doc> {
    /// # Safety
    /// `raw` must be null, or a live node that outlives `'doc` and belongs to a
    /// subtree still being built - nothing outside that subtree points at it.
    #[inline]
    pub(in crate::lexbor::adapter) unsafe fn from_raw(raw: *mut LxbNode) -> Option<Self> {
        HtmlNode::from_raw(raw).map(BuildingNode)
    }

    /// `node` as a handle for building under.
    ///
    /// # Safety
    /// As [`RawNode::as_node`], and `node` belongs to a subtree still being
    /// built - nothing outside that subtree points at it.
    #[inline]
    pub unsafe fn from_raw_node(node: RawNode) -> Self {
        BuildingNode(node.as_node())
    }

    /// The node as an ordinary handle, for reading.
    #[inline]
    pub fn node(self) -> HtmlNode<'doc> {
        self.0
    }

    #[inline]
    pub(in crate::lexbor::adapter) fn as_raw(self) -> *mut LxbNode {
        self.0.as_raw()
    }

    /// Forget the source position of this node and everything below it,
    /// `<template>` contents included: those are stamped at creation like any
    /// other element, and a deep copy carries them along.
    ///
    /// For a copy made from ANOTHER document: Lexbor's import copies `user`,
    /// and the offset in it indexes the source document's text, so `#line`
    /// answered with a line of this document the node was never on (41 in the
    /// source became 22 here). No position is the truthful answer - nil.
    /// Iterative, like every walk over a tree built from input.
    pub fn clear_source_offsets(self) {
        for n in self.0.subtree_with_contents() {
            n.forget_source_offset();
        }
    }

    /// Give this copy of `src` the name `src` records as written, which
    /// Lexbor's copy leaves behind (`lxb_dom_element_interface_copy` copies the
    /// tag, not the spelling): a copied SVG `linearGradient` read
    /// `lineargradient`, and `p:Bar` read `bar`. Nothing to do for a node
    /// with no written name. `Err` when Lexbor could not store it.
    pub fn copy_written_name_from(self, src: HtmlNode<'_>) -> Result<(), AdapterOom> {
        let (Some(from), Some(to)) = (src.element(), self.0.element()) else {
            return Ok(());
        };
        if !from.has_written_name() {
            return Ok(());
        }
        if src.owner_document() == self.0.owner_document() {
            /* One document, one tag table: the entry the source points at is
             * already this document's, so it is shared rather than looked up
             * again. Looking it up cost every copied SVG element a hash probe
             * (clone 10% slower, many small dups 20%) - and appending a name
             * the table already had can re-point its entry (see below). */
            // SAFETY: two live elements of one document; the tag entry is the
            // document's and outlives both.
            unsafe { (*to.raw()).qualified_name = (*from.raw()).qualified_name };
            return Ok(());
        }
        /* Another document's entry means nothing here, so the name is interned
         * in this one. A known gap, in Lexbor: `lxb_tag_append` given a name
         * the table already holds under ANOTHER tag - a parsed `<x:y>`, one
         * local name - re-points that entry, so the parsed element stops
         * matching CSS `x\:y`. It takes a prefixed `x:y` imported from another
         * document into one that already has a parsed `x:y`; see
         * NOKOGIRI_DIFFERENCES.md. */
        let name = from.qualified_name();
        // SAFETY: an element being built, in no tree yet; the name is copied
        // into this document's tag table.
        lexbor_ok(unsafe {
            lxb::lxb_dom_element_qualified_name_set(
                to.raw(),
                core::ptr::null(),
                0,
                name.as_ptr(),
                name.len(),
            )
        })
    }

    /// Where this node's CHILDREN attach: a `<template>`'s content fragment,
    /// the node itself otherwise.
    #[inline]
    pub fn link_target(self) -> Self {
        match self.0.template_content() {
            Some(content) => BuildingNode(content),
            None => self,
        }
    }

    /// This node's `<template>` contents fragment, still part of what is being
    /// built. `None` when the node is not an HTML `<template>`, and equally
    /// when it is one Lexbor gave no contents fragment.
    #[inline]
    pub fn template_content(self) -> Option<Self> {
        self.0.template_content().map(BuildingNode)
    }

    /// The next node in a walk of `root`'s subtree that also enters every
    /// `<template>`'s contents; see [`HtmlNode::preorder_next_with_contents`].
    #[inline]
    pub fn preorder_next_with_contents(self, root: Self) -> Option<Self> {
        self.0.preorder_next_with_contents(root.0).map(BuildingNode)
    }

    /// Put every element of this subtree - template contents included - that
    /// is in no namespace into `ns`, an id of this node's document.
    ///
    /// For a fragment parsed with no namespace in place of its context's (see
    /// `lexbor::fragment::FragmentTag::parse_ns`): the parser makes no
    /// element in no namespace except by inheriting the context's, so these
    /// are exactly the elements that inherited it. The struct Lexbor chose for
    /// each is the one it chooses for any namespace outside its built-in ones.
    pub fn give_namespace(self, ns: NsId) {
        let mut next = Some(self);
        while let Some(n) = next {
            if n.0.node_type() == NodeType::Element && n.0.ns_id().is_none() {
                // SAFETY: an element still being built, which nothing else
                // refers to; `ns` is interned in its document.
                unsafe { n.0.set_ns_id(ns) };
            }
            next = n.preorder_next_with_contents(self);
        }
    }

    /// Link `child` in as the last child.
    #[inline]
    pub fn insert_child(self, child: Self) {
        // SAFETY: two live nodes of one document, both still being built.
        unsafe { lxb::lxb_dom_node_insert_child(self.as_raw(), child.as_raw()) };
    }

    /// Link `node` in immediately before this one, under the same parent.
    #[inline]
    pub fn insert_before(self, node: Self) {
        // SAFETY: as `insert_child`.
        unsafe { lxb::lxb_dom_node_insert_before(self.as_raw(), node.as_raw()) };
    }
}

/// An element being filled in before anything links it into a tree.
///
/// Cross-document translation creates an element, copies the source's
/// attributes onto it, and hands it back for the caller to insert. It is NOT
/// [`HtmlElementMut`]: that type means the receiver passed the frozen and
/// evaluation checks, which say nothing about an element this code just made.
///
/// A half-built subtree that is abandoned on failure is left where it is: the
/// document's arena reclaims it wholesale, and nothing else ever points at it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BuildingElement<'doc>(HtmlElement<'doc>);

impl<'doc> BuildingElement<'doc> {
    /// The element as a node being built, for linking and for reading.
    #[inline]
    pub fn as_node(self) -> BuildingNode<'doc> {
        BuildingNode(self.0.node())
    }

    /// Put the element in the interned namespace `ns`.
    #[inline]
    pub fn set_ns(self, ns: NsId) {
        // SAFETY: an element nothing else holds; `ns` is an id this
        // document's own namespace table handed out.
        unsafe { self.0.node().set_ns_id(ns) };
    }

    /// DOM "append an attribute" of a new one named `qname` (case preserved)
    /// in `ns`, `None` for no namespace - a copy's step, so no existing
    /// attribute is looked for. `Err` when any step failed, in which case the
    /// unappended attribute is left for the arena, like the rest of an
    /// abandoned subtree.
    pub fn append_attribute(
        self,
        ns: Option<&[u8]>,
        qname: &[u8],
        value: &[u8],
    ) -> Result<(), AdapterOom> {
        // SAFETY: an element nothing else holds may be changed.
        unsafe { self.0.append_attribute(ns, qname, value) }
    }
}
