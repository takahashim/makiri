//! Editing a tree: the clearance types [`HtmlNodeMut`] and [`HtmlElementMut`],
//! and the rules an [`Insertion`] must keep before it is [placed](HtmlNodeMut::place).

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use super::*;

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

/// Why an insertion is refused (WHATWG DOM "ensure pre-insertion validity",
/// as Makiri applies it). Checked before any link changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreInsertError {
    /// A sibling place, or a replace, on a node with no parent.
    NoParent,
    /// An attribute node cannot be a child.
    AttributeNode,
    /// The node is the target or one of its ancestors.
    OwnSubtree,
    DoctypeParent,
    DuplicateDoctype,
    DoctypeAfterElement,
    ElementBeforeDoctype,
    /// The document already has its one element child.
    SecondDocumentElement,
    /// Text cannot be a child of the document.
    TextUnderDocument,
}

/// An insertion about to be made: `node` at `place` relative to `target`,
/// resolved to the parent it goes under, the child it goes before (none for an
/// append), and the child it replaces.
///
/// A value rather than loose arguments because the three positions are easy to
/// swap and mean different things - `before` bounds a scan, `replaces` is left
/// out of one - and the checks read them as a unit.
#[derive(Clone, Copy)]
pub struct Insertion<'d> {
    target: HtmlNode<'d>,
    parent: HtmlNode<'d>,
    before: Option<HtmlNode<'d>>,
    replaces: Option<HtmlNode<'d>>,
    node: HtmlNode<'d>,
}

impl<'d> Insertion<'d> {
    /// `node` at `place` relative to `target`; [`PreInsertError::NoParent`]
    /// for a place that needs `target`'s parent when it has none.
    pub fn new(
        target: HtmlNode<'d>,
        place: Place,
        node: HtmlNode<'d>,
    ) -> Result<Self, PreInsertError> {
        let parent = || target.parent().ok_or(PreInsertError::NoParent);
        let (parent, before, replaces) = match place {
            Place::Child => (target, None, None),
            Place::Before => (parent()?, Some(target), None),
            Place::After => (parent()?, target.next(), None),
            Place::Replace => (parent()?, Some(target), Some(target)),
        };
        Ok(Insertion {
            target,
            parent,
            before,
            replaces,
            node,
        })
    }

    /// Every rule the insertion must keep: the doctype ordering, then that the
    /// node can be a child at all and is not the target or its ancestor.
    pub fn check(&self) -> Result<(), PreInsertError> {
        self.check_document_order()?;
        if self.node.node_type() == TYPE_ATTRIBUTE {
            return Err(PreInsertError::AttributeNode);
        }
        /* The target itself counts: a node placed relative to itself would be
         * detached from the very position it is placed at. */
        if core::iter::successors(Some(self.target), |n| n.parent()).any(|n| n == self.node) {
            return Err(PreInsertError::OwnSubtree);
        }
        Ok(())
    }

    /// Whether `n` is a child that stays where it is: not the one being
    /// replaced, and not the incoming node (which may be moving within the same
    /// parent).
    fn stays(&self, n: HtmlNode<'d>) -> bool {
        Some(n) != self.replaces && n != self.node
    }

    /// The document's doctype/element ordering, checked before any link changes
    /// (WHATWG DOM "ensure pre-insertion validity", the doctype half).
    fn check_document_order(&self) -> Result<(), PreInsertError> {
        let siblings_from =
            |start: Option<HtmlNode<'d>>| core::iter::successors(start, |n| n.next());
        let at_document = self.parent.node_type() == TYPE_DOCUMENT;

        if self.node.node_type() == TYPE_DOCTYPE {
            if !at_document {
                return Err(PreInsertError::DoctypeParent);
            }
            /* At most one doctype ANYWHERE among the children. This scans the
             * whole list on purpose: stopping at `before` would let a node ahead
             * of the insertion point (a comment, say) hide a later doctype, and
             * the document would end up with two. */
            if siblings_from(self.parent.first_child())
                .any(|n| self.stays(n) && n.node_type() == TYPE_DOCTYPE)
            {
                return Err(PreInsertError::DuplicateDoctype);
            }
            /* No element before the insertion point. The scan stops AT `before`
             * before anything is excluded - on a replace `before` is also the
             * replaced node, and excluding it first would scan past it. */
            if siblings_from(self.parent.first_child())
                .take_while(|n| Some(*n) != self.before)
                .any(|n| self.stays(n) && n.node_type() == TYPE_ELEMENT)
            {
                return Err(PreInsertError::DoctypeAfterElement);
            }
            return Ok(());
        }

        let contributes_element = |n: HtmlNode<'_>| {
            n.node_type() == TYPE_ELEMENT
                || (n.node_type() == TYPE_FRAGMENT
                    && n.children().any(|child| child.node_type() == TYPE_ELEMENT))
        };
        /* An element must not land ahead of the doctype: none may follow the
         * insertion point. */
        if at_document
            && contributes_element(self.node)
            && siblings_from(self.before).any(|n| self.stays(n) && n.node_type() == TYPE_DOCTYPE)
        {
            return Err(PreInsertError::ElementBeforeDoctype);
        }
        if at_document {
            /* WHATWG DOM "ensure pre-insertion validity", the element half: a
             * document has at most one element child and no text child. Lexbor
             * enforces neither, so `doc << element` made a second root, and
             * `count(/child::*)` answered 2 - where the XML side refuses. After
             * the doctype order above, whose message an insertion breaking both
             * has always reported. */
            let is_text = |n: HtmlNode<'_>| matches!(n.node_type(), TYPE_TEXT | TYPE_CDATA);
            let is_element = |n: HtmlNode<'_>| n.node_type() == TYPE_ELEMENT;
            let incoming_elements = match self.node.node_type() {
                TYPE_FRAGMENT => {
                    if self.node.children().any(is_text) {
                        return Err(PreInsertError::TextUnderDocument);
                    }
                    self.node.children().filter(|&c| is_element(c)).count()
                }
                _ if is_text(self.node) => return Err(PreInsertError::TextUnderDocument),
                _ => usize::from(is_element(self.node)),
            };
            let has_element =
                siblings_from(self.parent.first_child()).any(|n| self.stays(n) && is_element(n));
            if incoming_elements > 1 || (incoming_elements == 1 && has_element) {
                return Err(PreInsertError::SecondDocumentElement);
            }
        }
        Ok(())
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

    #[inline]
    pub fn as_raw(self) -> *mut LxbNode {
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
    /// `false` when Lexbor could not store it, in which case the node keeps
    /// what it had.
    pub fn set_text_content(self, text: &[u8]) -> bool {
        let node = self.node();
        if matches!(node.node_type(), TYPE_ELEMENT | TYPE_FRAGMENT) {
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
                let Some(text_node) = node.owner_document().create_text(text) else {
                    return false;
                };
                Some(text_node)
            };
            while let Some(c) = self.first_child() {
                c.detach();
            }
            if let Some(text_node) = text_node {
                // SAFETY: a live node the caller may change, and a detached node
                // of its document just made.
                unsafe { lxb::lxb_dom_node_insert_child(self.as_raw(), text_node.as_raw()) };
            }
            return true;
        }
        // SAFETY: a live node the caller may change; for a character-data node
        // (or an attribute) Lexbor replaces the bytes in place and frees no node.
        let st =
            unsafe { lxb::lxb_dom_node_text_content_set(self.as_raw(), text.as_ptr(), text.len()) };
        st == lxb::consts::STATUS_OK
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
        if node.node().node_type() == TYPE_FRAGMENT {
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

    /// Set `name` to `value`, adding the attribute when the element has none.
    ///
    /// `None` when Lexbor could not store it. The lookup is Lexbor's own, by
    /// local name and lower-cased for HTML - `set_attribute_ns` is the one that
    /// keys on (namespace, local name) instead.
    pub fn set_attribute(self, name: &[u8], value: &[u8]) -> Option<HtmlAttr<'doc>> {
        self.0.put_attribute(name, value)
    }

    /// Create an attribute named `qname`, give it `value`, and append it, in
    /// namespace `ns` or none. `false` when any step failed.
    pub fn append_attribute(self, ns: Option<&[u8]>, qname: &[u8], value: &[u8]) -> bool {
        self.0.append_attribute_ns(ns, qname, value)
    }

    /// Take `attr` off the element. The arena keeps it, like a detached node.
    pub fn attr_remove(self, attr: HtmlAttr<'doc>) {
        // SAFETY: a live element the caller may change, and an attribute of it.
        unsafe { lxb::lxb_dom_element_attr_remove(self.0.raw(), attr.raw()) };
    }

    /// Take off the attribute Lexbor's lookup finds for `name`; no-op when
    /// there is none. Detached, as [`attr_remove`](Self::attr_remove) does -
    /// not `lxb_dom_element_remove_attribute`, which DESTROYS it while a Ruby
    /// wrapper may still hold it (the freed attribute's memory came back as a
    /// text node under the old Attr wrapper).
    pub fn remove_attribute(self, name: &[u8]) {
        // SAFETY: as above; the lookup only reads.
        let raw =
            unsafe { lxb::lxb_dom_element_attr_by_name(self.0.raw(), name.as_ptr(), name.len()) };
        if let Some(attr) = HtmlNode::link(raw as *mut LxbNode).and_then(HtmlNode::attr) {
            self.attr_remove(attr);
        }
    }
}
