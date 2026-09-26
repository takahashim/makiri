//! The WHATWG DOM's "ensure pre-insertion validity", written once for both
//! representations.
//!
//! The HTML adapter (Lexbor) and the XML arena each used to carry their own
//! copy of these rules, and the copies drifted: HTML let a node become a child
//! of a Text or a doctype and let a `<template>`'s contents fragment take in
//! its own template (a cycle every later deep walk looped on), while XML let
//! Text become a child of the Document and refused a DocumentFragment as a
//! parent. Now each representation describes its tree through [`Tree`] - the
//! same pattern as the XPath engine's `Dom` - and asks [`check`]. Linking, and
//! XML's namespace resolution, stay with the representation; only the decision
//! lives here.
//!
//! Lexbor/Ruby-free and safe, like the engine.
//!
//! One deliberate departure from the letter of the specification: the node
//! being inserted is never counted as one of `parent`'s existing children.
//! The DOM counts it, so moving a document's own root element before a comment
//! of the same document is a HierarchyRequestError there; both representations
//! have always allowed that move, and the result is a valid tree.

#![forbid(unsafe_code)]

use crate::node_type::NodeType;

/// A tree, as the insertion rules read it.
pub trait Tree {
    type Node: Copy + Eq;

    fn node_type(&self, n: Self::Node) -> NodeType;
    /// The node's parent in the tree - `None` for an attribute, whose owner
    /// element is not its parent here.
    fn tree_parent(&self, n: Self::Node) -> Option<Self::Node>;
    /// A DocumentFragment's host: an HTML `<template>` for its contents
    /// fragment. `None` for anything else, and always in XML.
    fn host(&self, n: Self::Node) -> Option<Self::Node>;
    fn first_child(&self, n: Self::Node) -> Option<Self::Node>;
    fn next_sibling(&self, n: Self::Node) -> Option<Self::Node>;
}

/// Why an insertion is refused, named after the DOM exception it raises there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    /// "NotFoundError": the reference child is not a child of the parent.
    NotFound,
    /// "HierarchyRequestError", with the rule that raised it.
    HierarchyRequest(Hierarchy),
}

/// Which rule a HierarchyRequestError comes from, in the order the DOM checks
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hierarchy {
    /// Step 1: only a Document, a DocumentFragment or an Element has children.
    ParentNotContainer,
    /// Step 2: the node is a host-including inclusive ancestor of the parent.
    Ancestor,
    /// Step 4: an attribute is not a tree child.
    AttributeNode,
    /// Step 4: a document is not a child.
    DocumentNode,
    /// Step 4: any other kind no DOM tree holds as a child.
    UnsupportedNode,
    /// Step 5 (and step 6 for a fragment): no Text child of a Document.
    TextUnderDocument,
    /// Step 5: a doctype is a child of a Document only.
    DoctypeParent,
    /// Step 6: a Document holds one doctype.
    DuplicateDoctype,
    /// Step 6: a doctype goes before the document element.
    DoctypeAfterElement,
    /// Step 6: an element goes after the doctype.
    ElementBeforeDoctype,
    /// Step 6: a Document holds one element.
    SecondDocumentElement,
}

impl From<Hierarchy> for Violation {
    fn from(h: Hierarchy) -> Self {
        Violation::HierarchyRequest(h)
    }
}

/// What an insertion does at its reference child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At<N> {
    /// Insert before this child; `None` appends.
    Before(Option<N>),
    /// Replace this child (the DOM's "replace a child", whose checks leave
    /// the replaced child out of the count).
    Replacing(N),
}

/// Would inserting `node` into `parent` at `at` keep the tree valid? The DOM's
/// "ensure pre-insertion validity", or for [`At::Replacing`] the same checks
/// as "replace a child" states them. Changes nothing, so a caller that checks
/// first and links second is all or nothing; a DocumentFragment is checked
/// with all its children at once.
pub fn check<T: Tree>(
    t: &T,
    parent: T::Node,
    node: T::Node,
    at: At<T::Node>,
) -> Result<(), Violation> {
    use Hierarchy as H;
    let ty = |n| t.node_type(n);
    let parent_ty = ty(parent);

    /* 1. */
    if !matches!(
        parent_ty,
        NodeType::Document | NodeType::DocumentFragment | NodeType::Element
    ) {
        return Err(H::ParentNotContainer.into());
    }
    /* 2. Host-including: a template's contents fragment has no parent, and
     * reaches its template through the host link - so a walk along parents
     * alone let the template go into its own contents. */
    let mut up = Some(parent);
    while let Some(cur) = up {
        if cur == node {
            return Err(H::Ancestor.into());
        }
        up = t.tree_parent(cur).or_else(|| t.host(cur));
    }
    /* 3. */
    let (child, replaced) = match at {
        At::Before(c) => (c, None),
        At::Replacing(c) => (Some(c), Some(c)),
    };
    if let Some(c) = child {
        if t.tree_parent(c) != Some(parent) {
            return Err(Violation::NotFound);
        }
    }
    /* 4. */
    let node_ty = ty(node);
    child_kind(node_ty)?;
    let is_text = |k| matches!(k, NodeType::Text | NodeType::CDataSection);
    let at_document = parent_ty == NodeType::Document;
    /* 5. */
    if is_text(node_ty) && at_document {
        return Err(H::TextUnderDocument.into());
    }
    if node_ty == NodeType::DocumentType && !at_document {
        return Err(H::DoctypeParent.into());
    }
    /* A fragment's children, which no DOM operation can make anything but
     * these kinds - checked anyway, since nothing below looks at a doctype
     * among them and it would land unordered. Fail closed. */
    if node_ty == NodeType::DocumentFragment {
        for c in children(t, node) {
            match ty(c) {
                NodeType::DocumentType => return Err(H::DoctypeParent.into()),
                k => child_kind(k)?,
            }
        }
    }
    if !at_document {
        return Ok(());
    }

    /* 6. The document's own children: `parent`'s, less the one being replaced
     * and the node itself (which may be moving within the document). */
    let stays = |n: T::Node| Some(n) != replaced && n != node;
    let existing = || children(t, parent).filter(move |&n| stays(n));
    let has = |k: NodeType| existing().any(|n| ty(n) == k);
    /* A doctype at or after the insertion point - `child` itself included
     * unless it is the one leaving. */
    let doctype_following = || {
        core::iter::successors(child, |&n| t.next_sibling(n))
            .any(|n| stays(n) && ty(n) == NodeType::DocumentType)
    };
    let incoming_elements = match node_ty {
        NodeType::DocumentFragment => {
            if children(t, node).any(|c| is_text(ty(c))) {
                return Err(H::TextUnderDocument.into());
            }
            children(t, node)
                .filter(|&c| ty(c) == NodeType::Element)
                .count()
        }
        NodeType::Element => 1,
        NodeType::DocumentType => {
            if has(NodeType::DocumentType) {
                return Err(H::DuplicateDoctype.into());
            }
            /* An element before the insertion point; with none (an append)
             * every element is before it. The scan stops AT `child` before
             * anything is left out - on a replace that is also the replaced
             * node, and leaving it out first would scan past it. */
            let element_before = children(t, parent)
                .take_while(|&n| Some(n) != child)
                .any(|n| stays(n) && ty(n) == NodeType::Element);
            return if element_before {
                Err(H::DoctypeAfterElement.into())
            } else {
                Ok(())
            };
        }
        _ => 0,
    };
    if incoming_elements == 0 {
        return Ok(());
    }
    /* The ordering first: an insertion that breaks both rules has always
     * been reported as this one. */
    if doctype_following() {
        return Err(H::ElementBeforeDoctype.into());
    }
    if incoming_elements > 1 || has(NodeType::Element) {
        return Err(H::SecondDocumentElement.into());
    }
    Ok(())
}

/// Step 4: whether a node of kind `k` can be a child at all.
fn child_kind(k: NodeType) -> Result<(), Violation> {
    match k {
        NodeType::DocumentFragment
        | NodeType::DocumentType
        | NodeType::Element
        | NodeType::Text
        | NodeType::CDataSection
        | NodeType::Pi
        | NodeType::Comment => Ok(()),
        NodeType::Attribute => Err(Hierarchy::AttributeNode.into()),
        NodeType::Document => Err(Hierarchy::DocumentNode.into()),
        _ => Err(Hierarchy::UnsupportedNode.into()),
    }
}

fn children<T: Tree>(t: &T, n: T::Node) -> impl Iterator<Item = T::Node> + '_ {
    core::iter::successors(t.first_child(n), move |&c| t.next_sibling(c))
}

#[cfg(test)]
mod tests;
