//! Where a node goes: the hierarchy rules, the four structural verbs, and
//! splicing a fragment's children.
//!
//! Every verb validates BEFORE it changes a link, so a refusal leaves the tree
//! exactly as it was - including a fragment, whose children are all checked
//! before any of them moves. The rules themselves are the WHATWG DOM's, shared
//! with the HTML adapter: [`crate::dom_rules::check`], over the [`Tree`] the
//! arena implements below.

#![forbid(unsafe_code)]

use super::ns::resolve_into;
use crate::dom_rules::{self, At, PreInsertError, Tree, Violation};
use crate::node_type::NodeType;
use crate::xml::{ArenaKind, Document, MutError, NodeId};

/// The three verbs that splice a fragment's children INTO an existing chain.
/// `Replace` is not one: it swaps the target out, which is
/// [`replace_with_fragment`]'s own shape - so the commit loop below cannot have
/// an arm for it, rather than having one that returns `Internal`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Splice {
    Child,
    Before,
    After,
}

/// Where [`place`] puts a node, relative to its target.
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

/// Put `node` at `place` relative to `target`. A DOCUMENT_FRAGMENT contributes
/// its CHILDREN, in order, and is left empty - as the DOM's insertion does -
/// and replacing with one swaps the target for all of them.
///
/// A fragment is ALL OR NOTHING: every child is validated before any link
/// changes. Inserting them one at a time was not, and the document node found
/// it - a two-element fragment appended there linked the first element, then
/// refused the second and raised, leaving the document holding half the
/// fragment (`spec/xml_fragment_spec.rb`). `Place::Replace` always validated
/// first; the other three now do too.
pub fn place(
    doc: &mut Document,
    target: NodeId,
    node: NodeId,
    place: Place,
) -> Result<(), MutError> {
    if doc.type_(node) != Some(ArenaKind::DocumentFragment) {
        return match place {
            Place::Child => insert_child(doc, target, node),
            Place::Before => insert_before(doc, target, node),
            Place::After => insert_after(doc, target, node),
            Place::Replace => replace_node(doc, target, node),
        };
    }
    match place {
        /* An empty fragment still removes the target. */
        Place::Replace => replace_with_fragment(doc, target, node),
        Place::Child => place_fragment(doc, target, node, Splice::Child),
        Place::Before => place_fragment(doc, target, node, Splice::Before),
        Place::After => place_fragment(doc, target, node, Splice::After),
    }
}

/// The site a spliced fragment's children go to.
fn splice_site(doc: &Document, target: NodeId, splice: Splice) -> Option<Site> {
    match splice {
        Splice::Child => Some(Site::appending(target)),
        Splice::Before => Some(Site::before(doc.tree_parent(target)?, target)),
        Splice::After => Some(Site::after(doc.tree_parent(target)?, target)),
    }
}

/// Splice every child of `frag` at `splice`, having already validated them.
fn place_fragment(
    doc: &mut Document,
    target: NodeId,
    frag: NodeId,
    splice: Splice,
) -> Result<(), MutError> {
    let Some(site) = splice_site(doc, target, splice) else {
        return Err(no_parent(false));
    };
    /* The fragment is checked as ONE node - the DOM's rules read its children
     * - and its subtree resolved as one, so every child passes before any
     * moves. */
    prepare_insert(doc, site, frag)?;
    /* --- commit pass: relinking only, so nothing here can refuse. `After` is
     * the one verb whose site MOVES - each child lands after the previous - so
     * it is rebuilt per child; the other two keep the validated one. */
    let mut last = target;
    while let Some(c) = doc.first_child(frag) {
        doc.detach(c);
        let here = match splice {
            Splice::After => Site::after(site.container, last),
            _ => site,
        };
        let (prev, next) = here.neighbours(doc);
        doc.splice_between(site.container, c, prev, next);
        last = c;
    }
    doc.sync_doc_meta(site.container);
    Ok(())
}

/// Unlink `node` from its parent.
pub fn detach(doc: &mut Document, node: NodeId) {
    doc.detach(node);
}

/// Which side of which child an insertion is anchored to.
///
/// `After` is NOT the same as `Before(next(target))`, and that mistake cost a
/// hang: the node being inserted may BE that next sibling, and `insert_at`
/// detaches it before reading the neighbours - which would leave the anchor
/// outside the chain and splice the node before ITSELF (`b.next == b`, a
/// self-referential sibling ring that every later walk loops on). An anchor on
/// the TARGET survives the detach, because the target is not the node moving.
#[derive(Clone, Copy)]
enum Anchor {
    /// At the end of the container's children.
    End,
    /// Immediately before this child.
    Before(NodeId),
    /// Immediately after this child.
    After(NodeId),
    /// In this child's place; it goes away.
    Replacing(NodeId),
}

/// Where an insertion goes, as the hierarchy rules see it: into `container`,
/// at `anchor` (which also says whether the insertion stands in for a child, so
/// that child does not count as an existing one). The rules read it through
/// [`Site::at`]; the linking through [`Site::neighbours`].
#[derive(Clone, Copy)]
struct Site {
    container: NodeId,
    anchor: Anchor,
}

impl Site {
    fn appending(container: NodeId) -> Site {
        Site {
            container,
            anchor: Anchor::End,
        }
    }
    fn before(container: NodeId, before: NodeId) -> Site {
        Site {
            container,
            anchor: Anchor::Before(before),
        }
    }
    fn after(container: NodeId, target: NodeId) -> Site {
        Site {
            container,
            anchor: Anchor::After(target),
        }
    }
    /// The site a `replace` leaves: `target`'s place, with `target` itself not
    /// counting as an existing child.
    fn replacing(container: NodeId, target: NodeId) -> Site {
        Site {
            container,
            anchor: Anchor::Replacing(target),
        }
    }

    /// The site as the DOM's rules read it: the reference child the insertion
    /// goes before (none for an append), or the one it replaces - read while
    /// the tree is still whole, before anything is detached.
    fn at(&self, doc: &Document) -> At<NodeId> {
        match self.anchor {
            Anchor::End => At::Before(None),
            Anchor::Before(r) => At::Before(Some(r)),
            Anchor::After(r) => At::Before(doc.next(r)),
            Anchor::Replacing(r) => At::Replacing(r),
        }
    }

    /// The two neighbours a node lands between here.
    ///
    /// The ONE statement of where an insertion goes; it used to be written twice,
    /// as a `(prev, next)` expression in each of the four verbs and again in the
    /// fragment commit loop.
    ///
    /// Read AFTER the incoming node is detached - a move can be its own
    /// neighbour, so the answer depends on the node already being out of the
    /// chain. Every arm anchors on a node that is NOT the one moving.
    fn neighbours(&self, doc: &Document) -> (Option<NodeId>, Option<NodeId>) {
        match self.anchor {
            Anchor::End => (doc.last_child(self.container), None),
            Anchor::Before(r) => (doc.prev(r), Some(r)),
            Anchor::After(r) => (Some(r), doc.next(r)),
            /* The target goes away, so its own next is the far side. */
            Anchor::Replacing(r) => (doc.prev(r), doc.next(r)),
        }
    }
}

/// The arena as [`dom_rules`] reads it. An attribute's stored parent is its
/// owner element, which is not a tree parent; and no XML fragment has a host.
impl Tree for Document {
    type Node = NodeId;
    #[inline]
    fn node_type(&self, n: NodeId) -> NodeType {
        self.type_(n).map_or(NodeType::Other, NodeType::from)
    }
    #[inline]
    fn tree_parent(&self, n: NodeId) -> Option<NodeId> {
        match self.type_(n)? {
            ArenaKind::Attribute => None,
            _ => Document::parent(self, n),
        }
    }
    #[inline]
    fn host(&self, _n: NodeId) -> Option<NodeId> {
        None
    }
    #[inline]
    fn first_child(&self, n: NodeId) -> Option<NodeId> {
        Document::first_child(self, n)
    }
    #[inline]
    fn last_child(&self, n: NodeId) -> Option<NodeId> {
        Document::last_child(self, n)
    }
    #[inline]
    fn next_sibling(&self, n: NodeId) -> Option<NodeId> {
        Document::next(self, n)
    }
    #[inline]
    fn prev_sibling(&self, n: NodeId) -> Option<NodeId> {
        Document::prev(self, n)
    }
}

/// A refused insertion as the mutators report it: with its rule, which the
/// bridge words as the HTML side does (`bridge::dom_error`).
fn refusal(v: Violation) -> MutError {
    MutError::PreInsert(PreInsertError::Rule(v))
}

/// The refusal of a sibling place (`replacing` false) or a replace on a node
/// with no tree parent.
fn no_parent(replacing: bool) -> MutError {
    MutError::PreInsert(PreInsertError::NoParent { replacing })
}

/// Validation + namespace resolution for inserting `node` at `site`. No
/// structural change.
///
/// A DOCUMENT_FRAGMENT is one node here: the rules read its children, and
/// [`resolve_into`] plans every element under it before writing any, so a
/// child whose prefix does not bind leaves its siblings undecided too. Under a
/// DETACHED fragment - a fragment is never connected - an unbound prefix stays
/// deferred, and resolves when the fragment is spliced into a document.
fn prepare_insert(doc: &mut Document, site: Site, node: NodeId) -> Result<(), MutError> {
    dom_rules::check(&*doc, site.container, node, site.at(doc)).map_err(refusal)?;
    resolve_into(doc, node, site.container)
}

/// Validate, then link `node` in at `site`. The shape every structural verb
/// shares; what differs between them is only the [`Site`] they build.
fn insert_at(doc: &mut Document, site: Site, node: NodeId) -> Result<(), MutError> {
    prepare_insert(doc, site, node)?;
    doc.detach(node);
    let (prev, next) = site.neighbours(doc);
    doc.splice_between(site.container, node, prev, next);
    doc.sync_doc_meta(site.container);
    Ok(())
}

pub fn insert_child(doc: &mut Document, parent: NodeId, node: NodeId) -> Result<(), MutError> {
    insert_at(doc, Site::appending(parent), node)
}

pub fn insert_before(doc: &mut Document, r: NodeId, node: NodeId) -> Result<(), MutError> {
    /* Beside itself is a no-op, not a self-loop. */
    if node == r {
        return Ok(());
    }
    /* The TREE parent, here and in every sibling verb: an attribute's stored
     * parent is its owner, and a node placed "beside" one was spliced into
     * the owner's children off the attribute's links - the attribute turned
     * up among the children and the element lost its old ones. */
    match doc.tree_parent(r) {
        Some(container) => insert_at(doc, Site::before(container, r), node),
        None => Err(no_parent(false)),
    }
}

pub fn insert_after(doc: &mut Document, r: NodeId, node: NodeId) -> Result<(), MutError> {
    if node == r {
        return Ok(());
    }
    match doc.tree_parent(r) {
        Some(container) => insert_at(doc, Site::after(container, r), node),
        None => Err(no_parent(false)),
    }
}

pub fn replace_node(doc: &mut Document, r: NodeId, node: NodeId) -> Result<(), MutError> {
    /* The parent check comes FIRST here, unlike the sibling verbs: replacing a
     * DETACHED node is a hierarchy error even when it is replaced by itself. */
    let Some(container) = doc.tree_parent(r) else {
        return Err(no_parent(true));
    };
    if node == r {
        return Ok(());
    }
    insert_at(doc, Site::replacing(container, r), node)?;
    /* `r`'s links now belong to `node`, so `detach` would unlink the wrong
     * node; the swapped-out one just forgets them. */
    doc.clear_links(r);
    doc.sync_doc_meta(container);
    Ok(())
}

/// Unlink `node` and re-derive the document meta it may have named.
pub fn remove(doc: &mut Document, node: NodeId) {
    let parent = doc.parent(node);
    doc.detach(node);
    if let Some(p) = parent {
        doc.sync_doc_meta(p);
    }
}

/// Replace `target` with the CHILDREN of `frag`, atomically (fail-closed).
pub fn replace_with_fragment(
    doc: &mut Document,
    target: NodeId,
    frag: NodeId,
) -> Result<(), MutError> {
    let Some(container) = doc.tree_parent(target) else {
        return Err(no_parent(true));
    };
    /* --- validation pass: no links change until it all passes */
    prepare_insert(doc, Site::replacing(container, target), frag)?;
    /* --- commit pass: every child takes target's slot in fragment order */
    while let Some(c) = doc.first_child(frag) {
        doc.detach(c);
        let prev = doc.prev(target);
        doc.splice_between(container, c, prev, Some(target));
    }
    remove(doc, target);
    Ok(())
}
