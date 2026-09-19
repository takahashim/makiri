//! Editing a tree: the clearance types [`HtmlNodeMut`] and [`HtmlElementMut`],
//! and the doctype-ordering rule an insertion into a document must keep.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use super::*;

/// Why an insertion would violate the document's required doctype ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentChildOrderError {
    DoctypeParent,
    DuplicateDoctype,
    DoctypeAfterElement,
    ElementBeforeDoctype,
}

/// Validate the HTML document's doctype/element ordering before an insertion.
/// `before` is None for append; `exclude` is a node replaced by this operation.
pub fn check_document_child_order(
    parent: Option<HtmlNode<'_>>,
    before: Option<HtmlNode<'_>>,
    exclude: Option<HtmlNode<'_>>,
    incoming: HtmlNode<'_>,
) -> Result<(), DocumentChildOrderError> {
    let contributes_element = |n: HtmlNode<'_>| {
        n.node_type() == TYPE_ELEMENT
            || (n.node_type() == TYPE_FRAGMENT
                && n.children().any(|child| child.node_type() == TYPE_ELEMENT))
    };
    if incoming.node_type() == TYPE_DOCTYPE {
        let Some(parent) = parent.filter(|p| p.node_type() == TYPE_DOCUMENT) else {
            return Err(DocumentChildOrderError::DoctypeParent);
        };
        /* At most one doctype ANYWHERE among the children. This scans the whole
         * list on purpose: stopping at `before` would let a node ahead of the
         * insertion point (a comment, say) hide a later doctype, and the
         * document would end up with two. */
        let mut cursor = parent.first_child();
        while let Some(node) = cursor {
            if Some(node) != exclude && node != incoming && node.node_type() == TYPE_DOCTYPE {
                return Err(DocumentChildOrderError::DuplicateDoctype);
            }
            cursor = node.next();
        }
        /* No element before the insertion point. `before` None is an append,
         * where every existing element precedes the new doctype. */
        let mut cursor = parent.first_child();
        while let Some(node) = cursor {
            if Some(node) == before {
                break;
            }
            if Some(node) != exclude && node != incoming && node.node_type() == TYPE_ELEMENT {
                return Err(DocumentChildOrderError::DoctypeAfterElement);
            }
            cursor = node.next();
        }
        return Ok(());
    }
    if contributes_element(incoming) && parent.is_some_and(|p| p.node_type() == TYPE_DOCUMENT) {
        let mut cursor = before;
        while let Some(node) = cursor {
            if Some(node) != exclude && node != incoming && node.node_type() == TYPE_DOCTYPE {
                return Err(DocumentChildOrderError::ElementBeforeDoctype);
            }
            cursor = node.next();
        }
    }
    Ok(())
}

/// A node the caller has cleared for editing.
///
/// Editing a tree is not something any handle should be able to do: a frozen
/// receiver must refuse, and a document an XPath handler is evaluating over must
/// refuse too, because the engine borrows names and index slices across the walk
/// (see `glue::doc::DocumentEvaluation`). Those two checks live in one place,
/// `glue::html_node::mutate::unwrap_mutable`, and this type is what that place
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
        // SAFETY: a live node the caller may change; Lexbor copies the bytes
        // into the document before anything else runs.
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
        // SAFETY: a live element the caller may change; both slices are read
        // and copied by Lexbor before anything else runs.
        let at = unsafe {
            lxb::lxb_dom_element_set_attribute(
                self.0.raw(),
                name.as_ptr(),
                name.len(),
                value.as_ptr(),
                value.len(),
            )
        };
        HtmlNode::link(at as *mut LxbNode).map(HtmlAttr)
    }

    /// Create an attribute named `qname`, give it `value`, and append it.
    ///
    /// `ns` is the namespace URI, or `None` for none - which is a different
    /// naming call, not an empty URI, so the two cannot be folded together. A
    /// fresh attribute is calloc'd into the null namespace already, so only the
    /// namespaced setter has to say anything about it.
    ///
    /// `false` when any step failed; the un-appended attribute is left for the
    /// document's arena to reclaim wholesale, this module's "never destroy"
    /// convention.
    pub fn append_attribute(self, ns: Option<&[u8]>, qname: &[u8], value: &[u8]) -> bool {
        // SAFETY: a live element of a live document the caller may change;
        // every slice is read and copied by Lexbor.
        unsafe {
            let at = lxb::lxb_dom_attr_interface_create(self.0.node().owner_document());
            let Some(at) = HtmlNode::link(at as *mut LxbNode).map(HtmlAttr) else {
                return false;
            };
            let named = match ns {
                Some(uri) => lxb::lxb_dom_attr_set_name_ns(
                    at.raw(),
                    uri.as_ptr(),
                    uri.len(),
                    qname.as_ptr(),
                    qname.len(),
                    false,
                ),
                None => lxb::lxb_dom_attr_set_name(at.raw(), qname.as_ptr(), qname.len(), false),
            };
            if named != lxb::consts::STATUS_OK || !at.set_value(value) {
                return false;
            }
            lxb::lxb_dom_element_attr_append(self.0.raw(), at.raw());
            true
        }
    }

    /// Take `attr` off the element. The arena keeps it, like a detached node.
    pub fn attr_remove(self, attr: HtmlAttr<'doc>) {
        // SAFETY: a live element the caller may change, and an attribute of it.
        unsafe { lxb::lxb_dom_element_attr_remove(self.0.raw(), attr.raw()) };
    }

    /// Remove the attribute Lexbor's lookup finds for `name`; no-op when there
    /// is none.
    pub fn remove_attribute(self, name: &[u8]) {
        // SAFETY: as above.
        unsafe {
            lxb::lxb_dom_element_remove_attribute(self.0.raw(), name.as_ptr(), name.len());
        }
    }
}
