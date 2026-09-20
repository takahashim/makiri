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
pub fn place(doc: &mut Document, target: NodeId, node: NodeId, place: Place) -> MutStatus {
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
        Splice::After => {
            let container = doc.parent(target)?;
            /* Every child lands before whatever follows the target, wherever
             * the moving insertion point has reached. */
            Some(match doc.next(target) {
                Some(next) => Site::before(container, next),
                None => Site::appending(container),
            })
        }
    }
}

/// The rule no per-child check can see: a Document holds ONE element, counting
/// the fragment's and the container's own together. Also refuses a DOCTYPE
/// child, which a fragment cannot hold today - a fragment is not an insertion
/// container - but which would otherwise be a silent second root-level doctype.
fn fragment_fits_container(doc: &Document, frag: NodeId, site: Site) -> MutStatus {
    if doc.type_(site.container) != Some(NodeType::Document) {
        return MutStatus::Ok;
    }
    if element_child_count(doc, frag, None) + element_child_count(doc, site.container, site.exclude)
        > 1
    {
        return MutStatus::Hierarchy;
    }
    let mut c = doc.first_child(frag);
    while let Some(cur) = c {
        if doc.type_(cur) == Some(NodeType::Doctype) {
            return MutStatus::Hierarchy;
        }
        c = doc.next(cur);
    }
    MutStatus::Ok
}

/// Validate every child of `frag` against `site` AND resolve its namespaces -
/// `prepare`, not `check`, because `prepare_insert` writes a resolved URI on a
/// node whose namespace is not yet decided. (A fragment's children always arrive
/// decided, from a parse or an import, so in practice nothing is written; the
/// name says what the code may do, not what it usually does.)
///
/// On `Ok` the commit that follows cannot fail: it only relinks.
fn prepare_fragment_children(doc: &mut Document, frag: NodeId, site: Site) -> MutStatus {
    let st = fragment_fits_container(doc, frag, site);
    if st != MutStatus::Ok {
        return st;
    }
    let mut c = doc.first_child(frag);
    while let Some(cur) = c {
        let st = prepare_insert(doc, site, cur);
        if st != MutStatus::Ok {
            return st;
        }
        c = doc.next(cur);
    }
    MutStatus::Ok
}

/// Splice every child of `frag` at `splice`, having already validated them.
fn place_fragment(doc: &mut Document, target: NodeId, frag: NodeId, splice: Splice) -> MutStatus {
    let Some(site) = splice_site(doc, target, splice) else {
        return MutStatus::Hierarchy;
    };
    let st = prepare_fragment_children(doc, frag, site);
    if st != MutStatus::Ok {
        return st;
    }
    /* --- commit pass: relinking only, so nothing here can refuse */
    let mut last = target; /* the moving insertion point, for After */
    while let Some(c) = doc.first_child(frag) {
        doc.detach(c);
        let (prev, next) = match splice {
            Splice::Child => (doc.last_child(site.container), None),
            Splice::Before => (doc.prev(target), Some(target)),
            Splice::After => {
                let after = (Some(last), doc.next(last));
                last = c;
                after
            }
        };
        doc.splice_between(site.container, c, prev, next);
    }
    doc.sync_doc_meta(site.container);
    MutStatus::Ok
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

/// Where an insertion goes, as the hierarchy rules see it: into `container`,
/// just before `before` (None = append), standing in for `exclude` (None = the
/// insertion replaces nothing, so every existing child counts).
///
/// The three used to travel as separate arguments through three predicates that
/// each walked the container's children again - up to four walks for one
/// insertion at the document node. Here they are one value and [`Site::check`]
/// is one walk.
#[derive(Clone, Copy)]
struct Site {
    container: NodeId,
    before: Option<NodeId>,
    exclude: Option<NodeId>,
}

impl Site {
    fn appending(container: NodeId) -> Site {
        Site {
            container,
            before: None,
            exclude: None,
        }
    }
    fn before(container: NodeId, before: NodeId) -> Site {
        Site {
            container,
            before: Some(before),
            exclude: None,
        }
    }
    /// The site a `replace` leaves: `target`'s place, with `target` itself not
    /// counting as an existing child.
    fn replacing(container: NodeId, target: NodeId) -> Site {
        Site {
            container,
            before: Some(target),
            exclude: Some(target),
        }
    }

    fn tally(&self, doc: &Document, node: NodeId) -> Tally {
        let mut t = Tally {
            elements: 0,
            doctypes: 0,
            element_before: false,
            doctype_at_or_after: false,
        };
        let mut reached = false;
        let mut c = doc.first_child(self.container);
        while let Some(cur) = c {
            if Some(cur) == self.before {
                reached = true;
            }
            if Some(cur) != self.exclude && cur != node {
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
            c = doc.next(cur);
        }
        t
    }

    /// The WHATWG document-child rules for `node` entering this site: at most
    /// one element and one doctype under a Document, the doctype before the
    /// element, and no doctype anywhere else. Fail-closed.
    fn check(&self, doc: &Document, node: NodeId) -> MutStatus {
        let ty = doc.type_(node);
        if doc.type_(self.container) != Some(NodeType::Document) {
            /* Only a Document may hold a doctype. */
            return if ty == Some(NodeType::Doctype) {
                MutStatus::Hierarchy
            } else {
                MutStatus::Ok
            };
        }
        match ty {
            Some(NodeType::Doctype) => {
                let t = self.tally(doc, node);
                if t.doctypes > 0 || t.element_before {
                    MutStatus::Hierarchy
                } else {
                    MutStatus::Ok
                }
            }
            Some(NodeType::Element) => {
                let t = self.tally(doc, node);
                if t.elements > 0 || t.doctype_at_or_after {
                    MutStatus::Hierarchy
                } else {
                    MutStatus::Ok
                }
            }
            _ => MutStatus::Ok,
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
fn prepare_insert(doc: &mut Document, site: Site, node: NodeId) -> MutStatus {
    if !is_insertable(doc, node) {
        return MutStatus::Hierarchy;
    }
    let ct = doc.type_(site.container);
    if ct != Some(NodeType::Element) && ct != Some(NodeType::Document) {
        return MutStatus::Hierarchy;
    }
    if would_cycle(doc, site.container, node) {
        return MutStatus::Cycle;
    }
    let st = site.check(doc, node);
    if st != MutStatus::Ok {
        return st;
    }
    resolve_into(doc, node, site.container)
}

pub fn insert_child(doc: &mut Document, parent: NodeId, node: NodeId) -> MutStatus {
    let st = prepare_insert(doc, Site::appending(parent), node);
    if st != MutStatus::Ok {
        return st;
    }
    doc.detach(node);
    let last = doc.last_child(parent);
    doc.splice_between(parent, node, last, None);
    doc.sync_doc_meta(parent);
    MutStatus::Ok
}

pub fn insert_before(doc: &mut Document, r: NodeId, node: NodeId) -> MutStatus {
    if node == r {
        return MutStatus::Ok;
    }
    let Some(container) = doc.parent(r) else {
        return MutStatus::Hierarchy;
    };
    let st = prepare_insert(doc, Site::before(container, r), node);
    if st != MutStatus::Ok {
        return st;
    }
    doc.detach(node);
    let prev = doc.prev(r);
    doc.splice_between(container, node, prev, Some(r));
    doc.sync_doc_meta(container);
    MutStatus::Ok
}

pub fn insert_after(doc: &mut Document, r: NodeId, node: NodeId) -> MutStatus {
    if node == r {
        return MutStatus::Ok;
    }
    let Some(container) = doc.parent(r) else {
        return MutStatus::Hierarchy;
    };
    let next = doc.next(r);
    let st = match next {
        Some(nx) => prepare_insert(doc, Site::before(container, nx), node),
        None => prepare_insert(doc, Site::appending(container), node),
    };
    if st != MutStatus::Ok {
        return st;
    }
    doc.detach(node);
    let next = doc.next(r);
    doc.splice_between(container, node, Some(r), next);
    doc.sync_doc_meta(container);
    MutStatus::Ok
}

pub fn replace_node(doc: &mut Document, r: NodeId, node: NodeId) -> MutStatus {
    let Some(container) = doc.parent(r) else {
        return MutStatus::Hierarchy;
    };
    if node == r {
        return MutStatus::Ok;
    }
    let st = prepare_insert(doc, Site::replacing(container, r), node);
    if st != MutStatus::Ok {
        return st;
    }
    doc.detach(node);
    let (prev, next) = (doc.prev(r), doc.next(r));
    doc.splice_between(container, node, prev, next);
    /* `r`'s links now belong to `node`, so `detach` would unlink the wrong
     * node; the swapped-out one just forgets them. */
    doc.clear_links(r);
    doc.sync_doc_meta(container);
    MutStatus::Ok
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
    let mut c = doc.first_child(parent);
    while let Some(cur) = c {
        if Some(cur) != exclude && doc.type_(cur) == Some(NodeType::Element) {
            n += 1;
        }
        c = doc.next(cur);
    }
    n
}

/// Replace `target` with the CHILDREN of `frag`, atomically (fail-closed).
pub fn replace_with_fragment(doc: &mut Document, target: NodeId, frag: NodeId) -> MutStatus {
    let Some(container) = doc.parent(target) else {
        return MutStatus::Hierarchy;
    };
    /* --- validation pass: no links change until it all passes */
    let st = prepare_fragment_children(doc, frag, Site::replacing(container, target));
    if st != MutStatus::Ok {
        return st;
    }
    /* --- commit pass: every child takes target's slot in fragment order */
    while let Some(c) = doc.first_child(frag) {
        doc.detach(c);
        let prev = doc.prev(target);
        doc.splice_between(container, c, prev, Some(target));
    }
    remove(doc, target);
    MutStatus::Ok
}
