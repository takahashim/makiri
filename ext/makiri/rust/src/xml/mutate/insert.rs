//! Where a node goes: the hierarchy rules, the four structural verbs, and
//! splicing a fragment's children.
//!
//! Every verb validates BEFORE it changes a link, so a refusal leaves the tree
//! exactly as it was - including a fragment, whose children are all checked
//! before any of them moves. The rules themselves are one [`Site`] and one walk
//! of the container's children ([`Site::check`]).

#![forbid(unsafe_code)]

use super::ns::resolve_into;
use crate::xml::{Document, MutStatus, NodeId, NodeType};

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
) -> Result<(), MutStatus> {
    if doc.type_(node) != Some(NodeType::Fragment) {
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
        Splice::Before => Some(Site::before(doc.parent(target)?, target)),
        Splice::After => Some(Site::after(doc.parent(target)?, target)),
    }
}

/// The rule no per-child check can see: a Document holds ONE element, counting
/// the fragment's and the container's own together. Also refuses a DOCTYPE
/// child, which a fragment cannot hold today - a fragment is not an insertion
/// container - but which would otherwise be a silent second root-level doctype.
fn fragment_fits_container(doc: &Document, frag: NodeId, site: Site) -> Result<(), MutStatus> {
    if doc.type_(site.container) != Some(NodeType::Document) {
        return Ok(());
    }
    if element_child_count(doc, frag, None)
        + element_child_count(doc, site.container, site.excluded())
        > 1
    {
        return Err(MutStatus::Hierarchy);
    }
    for cur in doc.children(frag) {
        if doc.type_(cur) == Some(NodeType::Doctype) {
            return Err(MutStatus::Hierarchy);
        }
    }
    Ok(())
}

/// Validate every child of `frag` against `site` AND resolve its namespaces -
/// `prepare`, not `check`, because `prepare_insert` writes a resolved URI on a
/// node whose namespace is not yet decided. (A fragment's children always arrive
/// decided, from a parse or an import, so in practice nothing is written; the
/// name says what the code may do, not what it usually does.)
///
/// On `Ok` the commit that follows cannot fail: it only relinks.
fn prepare_fragment_children(
    doc: &mut Document,
    frag: NodeId,
    site: Site,
) -> Result<(), MutStatus> {
    fragment_fits_container(doc, frag, site)?;
    let mut c = doc.first_child(frag);
    while let Some(cur) = c {
        prepare_insert(doc, site, cur)?;
        c = doc.next(cur);
    }
    Ok(())
}

/// Splice every child of `frag` at `splice`, having already validated them.
fn place_fragment(
    doc: &mut Document,
    target: NodeId,
    frag: NodeId,
    splice: Splice,
) -> Result<(), MutStatus> {
    let Some(site) = splice_site(doc, target, splice) else {
        return Err(MutStatus::Hierarchy);
    };
    prepare_fragment_children(doc, frag, site)?;
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

/// Unlink `node` from its parent. The invalid handle is a no-op.
pub fn detach(doc: &mut Document, node: NodeId) {
    if !node.is_invalid() {
        doc.detach(node);
    }
}

#[inline]
fn is_insertable(doc: &Document, node: NodeId) -> bool {
    matches!(
        doc.type_(node),
        Some(
            NodeType::Element
                | NodeType::Text
                | NodeType::CData
                | NodeType::Comment
                | NodeType::Pi
                | NodeType::Doctype
        )
    )
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
/// that child does not count as an existing one).
///
/// The parts used to travel as separate arguments through three predicates that
/// each walked the container's children again - up to four walks for one
/// insertion at the document node. Here they are one value and [`Site::check`]
/// is one walk.
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

    /// The child the insertion goes BEFORE, for the position-sensitive rules -
    /// read while the tree is still whole, before anything is detached.
    fn insertion_point(&self, doc: &Document) -> Option<NodeId> {
        match self.anchor {
            Anchor::End => None,
            Anchor::Before(r) | Anchor::Replacing(r) => Some(r),
            Anchor::After(r) => doc.next(r),
        }
    }

    /// The child that does not count as already being here: a `replace`'s
    /// target, which is on its way out.
    fn excluded(&self) -> Option<NodeId> {
        match self.anchor {
            Anchor::Replacing(r) => Some(r),
            _ => None,
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

    fn tally(&self, doc: &Document, node: NodeId) -> Tally {
        let mut t = Tally {
            elements: 0,
            doctypes: 0,
            element_before: false,
            doctype_at_or_after: false,
        };
        let before = self.insertion_point(doc);
        let exclude = self.excluded();
        let mut reached = false;
        for cur in doc.children(self.container) {
            if Some(cur) == before {
                reached = true;
            }
            if Some(cur) != exclude && cur != node {
                match doc.type_(cur) {
                    Some(NodeType::Element) => {
                        t.elements += 1;
                        if !reached {
                            t.element_before = true;
                        }
                    }
                    Some(NodeType::Doctype) => {
                        t.doctypes += 1;
                        if reached {
                            t.doctype_at_or_after = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        t
    }

    /// The WHATWG document-child rules for `node` entering this site: at most
    /// one element and one doctype under a Document, the doctype before the
    /// element, and no doctype anywhere else. Fail-closed.
    fn check(&self, doc: &Document, node: NodeId) -> Result<(), MutStatus> {
        let ty = doc.type_(node);
        if doc.type_(self.container) != Some(NodeType::Document) {
            /* Only a Document may hold a doctype. */
            return if ty == Some(NodeType::Doctype) {
                Err(MutStatus::Hierarchy)
            } else {
                Ok(())
            };
        }
        match ty {
            Some(NodeType::Doctype) => {
                let t = self.tally(doc, node);
                if t.doctypes > 0 || t.element_before {
                    Err(MutStatus::Hierarchy)
                } else {
                    Ok(())
                }
            }
            Some(NodeType::Element) => {
                let t = self.tally(doc, node);
                if t.elements > 0 || t.doctype_at_or_after {
                    Err(MutStatus::Hierarchy)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }
}

/// What the rules need to know about the container's existing children, counted
/// in one pass. "Before" and "at or after" are relative to [`Site::before`];
/// with no `before` nothing is ever reached, so every child counts as before it.
struct Tally {
    elements: usize,
    doctypes: usize,
    /// An element strictly before the insertion point.
    element_before: bool,
    /// A doctype at or after the insertion point.
    doctype_at_or_after: bool,
}

fn would_cycle(doc: &Document, container: NodeId, node: NodeId) -> bool {
    let mut p = Some(container);
    while let Some(cur) = p {
        if cur == node {
            return true;
        }
        p = doc.parent(cur);
    }
    false
}

/// Validation + namespace resolution for inserting `node` at `site`. No
/// structural change.
fn prepare_insert(doc: &mut Document, site: Site, node: NodeId) -> Result<(), MutStatus> {
    if !is_insertable(doc, node) {
        return Err(MutStatus::Hierarchy);
    }
    let ct = doc.type_(site.container);
    if ct != Some(NodeType::Element) && ct != Some(NodeType::Document) {
        return Err(MutStatus::Hierarchy);
    }
    if would_cycle(doc, site.container, node) {
        return Err(MutStatus::Cycle);
    }
    site.check(doc, node)?;
    resolve_into(doc, node, site.container)
}

/// Validate, then link `node` in at `site`. The shape every structural verb
/// shares; what differs between them is only the [`Site`] they build.
fn insert_at(doc: &mut Document, site: Site, node: NodeId) -> Result<(), MutStatus> {
    prepare_insert(doc, site, node)?;
    doc.detach(node);
    let (prev, next) = site.neighbours(doc);
    doc.splice_between(site.container, node, prev, next);
    doc.sync_doc_meta(site.container);
    Ok(())
}

pub fn insert_child(doc: &mut Document, parent: NodeId, node: NodeId) -> Result<(), MutStatus> {
    insert_at(doc, Site::appending(parent), node)
}

pub fn insert_before(doc: &mut Document, r: NodeId, node: NodeId) -> Result<(), MutStatus> {
    /* Beside itself is a no-op, not a self-loop. */
    if node == r {
        return Ok(());
    }
    match doc.parent(r) {
        Some(container) => insert_at(doc, Site::before(container, r), node),
        None => Err(MutStatus::Hierarchy),
    }
}

pub fn insert_after(doc: &mut Document, r: NodeId, node: NodeId) -> Result<(), MutStatus> {
    if node == r {
        return Ok(());
    }
    match doc.parent(r) {
        Some(container) => insert_at(doc, Site::after(container, r), node),
        None => Err(MutStatus::Hierarchy),
    }
}

pub fn replace_node(doc: &mut Document, r: NodeId, node: NodeId) -> Result<(), MutStatus> {
    /* The parent check comes FIRST here, unlike the sibling verbs: replacing a
     * DETACHED node is a hierarchy error even when it is replaced by itself. */
    let Some(container) = doc.parent(r) else {
        return Err(MutStatus::Hierarchy);
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

/// Unlink `node` and re-derive the document meta it may have named. The invalid
/// handle is a no-op.
pub fn remove(doc: &mut Document, node: NodeId) {
    if node.is_invalid() {
        return;
    }
    let parent = doc.parent(node);
    doc.detach(node);
    if let Some(p) = parent {
        doc.sync_doc_meta(p);
    }
}

fn element_child_count(doc: &Document, parent: NodeId, exclude: Option<NodeId>) -> usize {
    let mut n = 0;
    for cur in doc.children(parent) {
        if Some(cur) != exclude && doc.type_(cur) == Some(NodeType::Element) {
            n += 1;
        }
    }
    n
}

/// Replace `target` with the CHILDREN of `frag`, atomically (fail-closed).
pub fn replace_with_fragment(
    doc: &mut Document,
    target: NodeId,
    frag: NodeId,
) -> Result<(), MutStatus> {
    let Some(container) = doc.parent(target) else {
        return Err(MutStatus::Hierarchy);
    };
    /* --- validation pass: no links change until it all passes */
    prepare_fragment_children(doc, frag, Site::replacing(container, target))?;
    /* --- commit pass: every child takes target's slot in fragment order */
    while let Some(c) = doc.first_child(frag) {
        doc.detach(c);
        let prev = doc.prev(target);
        doc.splice_between(container, c, prev, Some(target));
    }
    remove(doc, target);
    Ok(())
}
