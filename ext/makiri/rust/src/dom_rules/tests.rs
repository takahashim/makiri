//! Each DOM rule against a tiny tree that implements [`Tree`] and nothing else.

use super::*;

/// Nodes are indices; `parent` is the tree parent (never set for an
/// attribute), `host` a fragment's template.
#[derive(Default)]
struct Mini {
    ty: Vec<NodeType>,
    parent: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
    host: Vec<Option<usize>>,
}

impl Mini {
    fn add(&mut self, ty: NodeType, parent: Option<usize>) -> usize {
        let id = self.ty.len();
        self.ty.push(ty);
        self.parent.push(parent);
        self.children.push(Vec::new());
        self.host.push(None);
        if let Some(p) = parent {
            self.children[p].push(id);
        }
        id
    }
}

impl Tree for Mini {
    type Node = usize;
    fn node_type(&self, n: usize) -> NodeType {
        self.ty[n]
    }
    fn tree_parent(&self, n: usize) -> Option<usize> {
        self.parent[n]
    }
    fn host(&self, n: usize) -> Option<usize> {
        self.host[n]
    }
    fn first_child(&self, n: usize) -> Option<usize> {
        self.children[n].first().copied()
    }
    fn next_sibling(&self, n: usize) -> Option<usize> {
        let p = self.parent[n]?;
        let s = &self.children[p];
        let i = s.iter().position(|&c| c == n)?;
        s.get(i + 1).copied()
    }
}

use NodeType as K;

fn hier(h: Hierarchy) -> Result<(), Violation> {
    Err(Violation::HierarchyRequest(h))
}

/// A document holding `<!DOCTYPE>`, a comment and `<html>`, plus a detached
/// fragment and element.
struct Doc {
    t: Mini,
    doc: usize,
    doctype: usize,
    comment: usize,
    html: usize,
    body: usize,
}

fn doc() -> Doc {
    let mut t = Mini::default();
    let doc = t.add(K::Document, None);
    let doctype = t.add(K::DocumentType, Some(doc));
    let comment = t.add(K::Comment, Some(doc));
    let html = t.add(K::Element, Some(doc));
    let body = t.add(K::Element, Some(html));
    Doc {
        t,
        doc,
        doctype,
        comment,
        html,
        body,
    }
}

const APPEND: At<usize> = At::Before(None);

#[test]
fn step1_only_document_fragment_and_element_take_children() {
    let mut d = doc();
    let el = d.t.add(K::Element, None);
    for k in [
        K::Text,
        K::CDataSection,
        K::Comment,
        K::Pi,
        K::DocumentType,
        K::Attribute,
    ] {
        let p = d.t.add(k, None);
        assert_eq!(
            check(&d.t, p, el, APPEND),
            hier(Hierarchy::ParentNotContainer),
            "{k:?}"
        );
    }
    let frag = d.t.add(K::DocumentFragment, None);
    assert_eq!(check(&d.t, frag, el, APPEND), Ok(()));
    assert_eq!(check(&d.t, d.body, el, APPEND), Ok(()));
}

#[test]
fn step2_no_node_goes_under_itself() {
    let d = doc();
    assert_eq!(
        check(&d.t, d.body, d.body, APPEND),
        hier(Hierarchy::Ancestor)
    );
    assert_eq!(
        check(&d.t, d.body, d.html, APPEND),
        hier(Hierarchy::Ancestor)
    );
}

#[test]
fn step2_walks_through_a_template_host() {
    let mut d = doc();
    let template = d.t.add(K::Element, Some(d.body));
    let content = d.t.add(K::DocumentFragment, None);
    d.t.host[content] = Some(template);
    let inner = d.t.add(K::Element, Some(content));
    assert_eq!(
        check(&d.t, content, template, APPEND),
        hier(Hierarchy::Ancestor)
    );
    assert_eq!(
        check(&d.t, inner, template, APPEND),
        hier(Hierarchy::Ancestor)
    );
    assert_eq!(
        check(&d.t, inner, d.body, APPEND),
        hier(Hierarchy::Ancestor)
    );
    /* A sibling of the template is fine. */
    let other = d.t.add(K::Element, None);
    assert_eq!(check(&d.t, content, other, APPEND), Ok(()));
}

#[test]
fn step3_the_reference_child_must_be_a_child_of_parent() {
    let mut d = doc();
    let el = d.t.add(K::Element, None);
    assert_eq!(
        check(&d.t, d.html, el, At::Before(Some(d.comment))),
        Err(Violation::NotFound)
    );
    assert_eq!(
        check(&d.t, d.html, el, At::Replacing(d.html)),
        Err(Violation::NotFound)
    );
    /* An attribute has no tree parent, so it is nobody's reference child. */
    let attr = d.t.add(K::Attribute, None);
    assert_eq!(
        check(&d.t, d.html, el, At::Before(Some(attr))),
        Err(Violation::NotFound)
    );
    assert_eq!(check(&d.t, d.html, el, At::Before(Some(d.body))), Ok(()));
}

#[test]
fn step4_only_child_kinds_are_inserted() {
    let mut d = doc();
    let attr = d.t.add(K::Attribute, None);
    let other_doc = d.t.add(K::Document, None);
    let entity = d.t.add(K::EntityReference, None);
    assert_eq!(
        check(&d.t, d.body, attr, APPEND),
        hier(Hierarchy::AttributeNode)
    );
    assert_eq!(
        check(&d.t, d.body, other_doc, APPEND),
        hier(Hierarchy::DocumentNode)
    );
    assert_eq!(
        check(&d.t, d.body, entity, APPEND),
        hier(Hierarchy::UnsupportedNode)
    );
    for k in [K::Text, K::CDataSection, K::Comment, K::Pi, K::Element] {
        let n = d.t.add(k, None);
        assert_eq!(check(&d.t, d.body, n, APPEND), Ok(()), "{k:?}");
    }
}

#[test]
fn step5_text_is_not_a_document_child_and_a_doctype_is_nothing_else() {
    let mut d = doc();
    for k in [K::Text, K::CDataSection] {
        let n = d.t.add(k, None);
        assert_eq!(
            check(&d.t, d.doc, n, APPEND),
            hier(Hierarchy::TextUnderDocument)
        );
    }
    let dt = d.t.add(K::DocumentType, None);
    assert_eq!(
        check(&d.t, d.body, dt, APPEND),
        hier(Hierarchy::DoctypeParent)
    );
    let frag = d.t.add(K::DocumentFragment, None);
    assert_eq!(
        check(&d.t, frag, dt, APPEND),
        hier(Hierarchy::DoctypeParent)
    );
    /* Comments and PIs are fine at document level. */
    let c = d.t.add(K::Comment, None);
    assert_eq!(check(&d.t, d.doc, c, APPEND), Ok(()));
}

#[test]
fn step6_element_one_root_after_the_doctype() {
    let mut d = doc();
    let el = d.t.add(K::Element, None);
    assert_eq!(
        check(&d.t, d.doc, el, APPEND),
        hier(Hierarchy::SecondDocumentElement)
    );
    assert_eq!(
        check(&d.t, d.doc, el, At::Before(Some(d.doctype))),
        hier(Hierarchy::ElementBeforeDoctype)
    );
    /* Replacing the root is fine; replacing the doctype puts it first. */
    assert_eq!(check(&d.t, d.doc, el, At::Replacing(d.html)), Ok(()));
    assert_eq!(
        check(&d.t, d.doc, el, At::Replacing(d.comment)),
        hier(Hierarchy::SecondDocumentElement)
    );
    /* The root moving within its own document is not a second root. */
    assert_eq!(
        check(&d.t, d.doc, d.html, At::Before(Some(d.comment))),
        Ok(())
    );
    assert_eq!(
        check(&d.t, d.doc, d.html, At::Before(Some(d.doctype))),
        hier(Hierarchy::ElementBeforeDoctype)
    );
}

#[test]
fn step6_doctype_one_before_the_element() {
    let mut d = doc();
    let dt = d.t.add(K::DocumentType, None);
    assert_eq!(
        check(&d.t, d.doc, dt, APPEND),
        hier(Hierarchy::DuplicateDoctype)
    );
    assert_eq!(check(&d.t, d.doc, dt, At::Replacing(d.doctype)), Ok(()));
    assert_eq!(
        check(&d.t, d.doc, dt, At::Replacing(d.html)),
        hier(Hierarchy::DuplicateDoctype)
    );

    let mut t = Mini::default();
    let doc = t.add(K::Document, None);
    let c = t.add(K::Comment, Some(doc));
    let root = t.add(K::Element, Some(doc));
    let dt = t.add(K::DocumentType, None);
    assert_eq!(check(&t, doc, dt, At::Before(Some(c))), Ok(()));
    assert_eq!(check(&t, doc, dt, At::Before(Some(root))), Ok(()));
    assert_eq!(
        check(&t, doc, dt, APPEND),
        hier(Hierarchy::DoctypeAfterElement)
    );
    /* Replacing the root: the root leaves, so nothing precedes. */
    assert_eq!(check(&t, doc, dt, At::Replacing(root)), Ok(()));
}

#[test]
fn step6_fragment_into_a_document() {
    let mut t = Mini::default();
    let doc = t.add(K::Document, None);
    let dt = t.add(K::DocumentType, Some(doc));
    let frag = t.add(K::DocumentFragment, None);
    t.add(K::Comment, Some(frag));
    let a = t.add(K::Element, Some(frag));
    assert_eq!(check(&t, doc, frag, APPEND), Ok(()));
    assert_eq!(
        check(&t, doc, frag, At::Before(Some(dt))),
        hier(Hierarchy::ElementBeforeDoctype)
    );
    t.add(K::Element, Some(frag));
    assert_eq!(
        check(&t, doc, frag, APPEND),
        hier(Hierarchy::SecondDocumentElement)
    );
    t.children[frag].retain(|&c| c == a);
    t.add(K::Text, Some(frag));
    assert_eq!(
        check(&t, doc, frag, APPEND),
        hier(Hierarchy::TextUnderDocument)
    );

    /* Under an element, anything a fragment holds is fine - but never a
     * doctype, which no DOM path puts there. */
    let el = t.add(K::Element, None);
    assert_eq!(check(&t, el, frag, APPEND), Ok(()));
    t.add(K::DocumentType, Some(frag));
    assert_eq!(check(&t, el, frag, APPEND), hier(Hierarchy::DoctypeParent));
}

#[test]
fn step6_existing_root_blocks_a_fragment_element() {
    let mut d = doc();
    let frag = d.t.add(K::DocumentFragment, None);
    d.t.add(K::Element, Some(frag));
    assert_eq!(
        check(&d.t, d.doc, frag, APPEND),
        hier(Hierarchy::SecondDocumentElement)
    );
    assert_eq!(check(&d.t, d.doc, frag, At::Replacing(d.html)), Ok(()));
    /* An empty fragment adds nothing. */
    let empty = d.t.add(K::DocumentFragment, None);
    assert_eq!(check(&d.t, d.doc, empty, APPEND), Ok(()));
}

/// `root`: the end of the tree-parent chain. A fragment is a root even with a
/// host (the host is not its parent), and an attribute is its own root.
#[test]
fn root_ends_the_tree_parent_chain() {
    let d = doc();
    let mut t = d.t;
    assert_eq!(root(&t, d.body), d.doc);
    assert_eq!(root(&t, d.doc), d.doc);

    let template = t.add(K::Element, Some(d.body));
    let contents = t.add(K::DocumentFragment, None);
    t.host[contents] = Some(template);
    let inside = t.add(K::Element, Some(contents));
    assert_eq!(root(&t, inside), contents);

    let detached = t.add(K::Element, None);
    let leaf = t.add(K::Text, Some(detached));
    assert_eq!(root(&t, leaf), detached);

    let attr = t.add(K::Attribute, None);
    assert_eq!(root(&t, attr), attr);
}
