//! Mutation primitives. Every primitive validates and allocates BEFORE
//! changing any link, so a failure leaves the tree untouched.
//!
//! The tree is an index arena, so this module is ordinary safe Rust under
//! `#![forbid(unsafe_code)]`: nodes are [`NodeId`] values, structure lives in
//! the [`Document`], and names/values are spans into its byte store.

#![forbid(unsafe_code)]

use crate::falloc::Reserve;
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, xmlns_prefix, Split};
use crate::xml::{
    Document, Link, MutStatus, NodeId, NodeType, Span, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED,
};

/// Whether `text` is free of the SEQUENCE its node kind cannot hold: "--" (or
/// a trailing "-") in a comment, "]]>" in CDATA, "?>" in a PI. Each would close
/// the construct early, so the value is refused rather than escaped.
///
/// A mutation precondition, not a naming rule: the parser never needs it,
/// because it finds those sequences structurally while scanning.
fn value_seq_ok(node_type: NodeType, text: &[u8]) -> bool {
    match node_type {
        NodeType::Comment => text.last() != Some(&b'-') && !text.windows(2).any(|w| w == b"--"),
        NodeType::CData => !text.windows(3).any(|w| w == b"]]>"),
        NodeType::Pi => !text.windows(2).any(|w| w == b"?>"),
        _ => true,
    }
}

/// A resolved namespace: a byte-store span (empty = no namespace).
type Ns = Span;

const NO_NS: Ns = Span::EMPTY;

/// Copy a node's span out of the arena before taking `&mut doc`. `to_vec` would
/// abort on OOM; this path must fail closed instead, like every other
/// allocation here.
fn copy_span(bytes: &[u8]) -> Result<Vec<u8>, MutStatus> {
    crate::falloc::try_to_vec(bytes).ok_or(MutStatus::Oom)
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
    /* An empty fragment still removes the target. */
    if place == Place::Replace {
        return replace_with_fragment(doc, target, node);
    }
    place_fragment(doc, target, node, place)
}

/// The site a fragment's children go to, and the node they stand in for.
fn fragment_site(doc: &Document, target: NodeId, place: Place) -> Option<Site> {
    match place {
        Place::Child => Some(Site::appending(target)),
        Place::Before => Some(Site::before(doc.parent(target)?, target)),
        Place::After => {
            let container = doc.parent(target)?;
            /* Every child lands before whatever follows the target, wherever
             * the moving insertion point has reached. */
            Some(match doc.next(target) {
                Some(next) => Site::before(container, next),
                None => Site::appending(container),
            })
        }
        Place::Replace => Some(Site::replacing(doc.parent(target)?, target)),
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

/// Validate every child of `frag` against `site`. On `Ok` the commit that
/// follows cannot fail: it only relinks.
fn check_fragment_children(doc: &mut Document, frag: NodeId, site: Site) -> MutStatus {
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

/// Splice every child of `frag` at `place`, having already validated them.
fn place_fragment(doc: &mut Document, target: NodeId, frag: NodeId, place: Place) -> MutStatus {
    let Some(site) = fragment_site(doc, target, place) else {
        return MutStatus::Hierarchy;
    };
    let st = check_fragment_children(doc, frag, site);
    if st != MutStatus::Ok {
        return st;
    }
    /* --- commit pass: relinking only, so nothing here can refuse */
    let mut last = target; /* the moving insertion point, for After */
    while let Some(c) = doc.first_child(frag) {
        doc.detach(c);
        match place {
            Place::Child => {
                let prev = doc.last_child(site.container);
                doc.splice_between(site.container, c, prev, None);
            }
            Place::Before => {
                let prev = doc.prev(target);
                doc.splice_between(site.container, c, prev, Some(target));
            }
            Place::After => {
                let next = doc.next(last);
                doc.splice_between(site.container, c, Some(last), next);
                last = c;
            }
            /* handled by replace_with_fragment */
            Place::Replace => return MutStatus::Internal,
        }
    }
    doc.sync_doc_meta(site.container);
    MutStatus::Ok
}

/// Resolve `name` (split per `sp`) applied at `scope` (mirrors the parser's §7
/// rules). An unbound prefix is an error only when connected; deferred
/// (unresolved) otherwise.
fn resolve_ns(
    doc: &Document,
    scope: Option<NodeId>,
    name: &[u8],
    sp: &Split,
    is_attr: bool,
    connected: bool,
) -> Result<Ns, MutStatus> {
    let prefix = &name[..sp.prefix_len as usize];
    if is_attr && xmlns_prefix(name).is_some() {
        return Ok(doc.xmlns_ns_span());
    }
    if sp.prefix_len == 0 {
        if is_attr {
            return Ok(NO_NS); /* unprefixed attribute -> no namespace */
        }
        let s = doc.resolve_in_scope(scope, b"");
        return Ok(if s.len > 0 { s } else { NO_NS });
    }
    if prefix == b"xml" {
        return Ok(doc.xml_ns_span());
    }
    if prefix == b"xmlns" {
        return Err(MutStatus::BadName);
    }
    let s = doc.resolve_in_scope(scope, prefix);
    if s.len > 0 {
        Ok(s)
    } else if connected {
        Err(MutStatus::UnboundNs)
    } else {
        Ok(NO_NS)
    }
}

#[inline]
fn assign_qname(doc: &mut Document, node: NodeId, name: &[u8], sp: &Split) -> MutStatus {
    if doc
        .assign_qname(node, name, sp.prefix_len, sp.local_off, sp.local_len)
        .is_ok()
    {
        MutStatus::Ok
    } else {
        MutStatus::Oom
    }
}

/// Unlink `node` from its parent. The invalid handle is a no-op.
pub fn detach(doc: &mut Document, node: NodeId) {
    if !node.is_invalid() {
        doc.detach(node);
    }
}

pub fn rename(doc: &mut Document, node: NodeId, name: &[u8]) -> MutStatus {
    if doc.type_(node) != Some(NodeType::Element) && doc.type_(node) != Some(NodeType::Attribute) {
        return MutStatus::Type;
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MutStatus::BadName,
    };
    let is_attr = doc.type_(node) == Some(NodeType::Attribute);
    let scope: Option<NodeId> = if is_attr {
        doc.parent(node)
    } else {
        Some(node)
    };
    let connected = scope.is_some_and(|s| doc.is_connected(s));
    let ns = match resolve_ns(doc, scope, name, &sp, is_attr, connected) {
        Ok(ns) => ns,
        Err(st) => return st,
    };
    /* copy the new qname BEFORE writing ns_uri, so an OOM leaves node intact */
    let st = assign_qname(doc, node, name, &sp);
    if st != MutStatus::Ok {
        return st;
    }
    {
        let n = doc.node_mut(node);
        n.ns_uri = ns;
        n.flags &= !FLAG_DOM_LOOSE_NAME;
    }
    /* A rename picks a new prefix, so it decides a new URI from the scope the
     * node is in right now - and that decision is the node's identity from here
     * (an element's; an attribute follows its element). */
    if connected && !is_attr {
        doc.node_mut(node).flags |= FLAG_NS_RESOLVED;
    }
    MutStatus::Ok
}

/// Build a fresh ATTRIBUTE (qname + value + namespace) and link it onto `el`
/// after `tail`, the last entry the caller's own scan reached.
fn build_attr(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    sp: &Split,
    val: &[u8],
    ns: Ns,
    tail: Option<NodeId>,
) -> Result<NodeId, MutStatus> {
    let attr = doc
        .new_node(NodeType::Attribute)
        .map_err(|_| MutStatus::Oom)?;
    let st = assign_qname(doc, attr, name, sp);
    if st != MutStatus::Ok {
        return Err(st);
    }
    doc.set_value_bytes(attr, val).map_err(|_| MutStatus::Oom)?;
    doc.node_mut(attr).ns_uri = ns;
    doc.link_attr(el, tail, attr);
    Ok(attr)
}

pub fn set_attribute(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutStatus> {
    if doc.type_(el) != Some(NodeType::Element) {
        return Err(MutStatus::Type);
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutStatus::BadName),
    };
    /* xmlns:foo="" must not bind a prefix to the empty namespace */
    if val.is_empty() && sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MutStatus::BadNsDecl);
    }
    if !val.is_empty() && !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let connected = doc.is_connected(el);
    let ns = resolve_ns(doc, Some(el), name, &sp, true, connected)?;
    /* an existing attribute with the same raw QName -> replace its value */
    let mut tail = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if doc.qname(attr) == name {
            doc.set_value_bytes(attr, val).map_err(|_| MutStatus::Oom)?;
            doc.node_mut(attr).ns_uri = ns;
            return Ok(attr);
        }
        tail = Some(attr);
        a = doc.next(attr);
    }
    build_attr(doc, el, name, &sp, val, ns, tail)
}

/// Remove `el`'s attribute named `name`; `true` when one was removed.
pub fn remove_attribute(doc: &mut Document, el: NodeId, name: &[u8]) -> bool {
    if doc.type_(el) != Some(NodeType::Element) {
        return false;
    }
    let mut prev: Option<NodeId> = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if doc.qname(attr) == name {
            doc.unlink_attr(el, prev, attr);
            return true;
        }
        prev = Some(attr);
        a = doc.next(attr);
    }
    false
}

/// `a` is keyed by (ns, local) - the DOM key; an empty wanted namespace
/// matches an attribute with no namespace.
fn attr_matches_ns(doc: &Document, a: NodeId, ns: &[u8], local: &[u8]) -> bool {
    doc.node(a).ns_uri.len as usize == ns.len()
        && (ns.is_empty() || doc.ns(a) == ns)
        && doc.local(a) == local
}

pub fn set_attribute_ns(
    doc: &mut Document,
    el: NodeId,
    ns: &[u8],
    name: &[u8],
    val: &[u8],
) -> Result<NodeId, MutStatus> {
    if doc.type_(el) != Some(NodeType::Element) {
        return Err(MutStatus::Type);
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutStatus::BadName),
    };
    if !val.is_empty() && !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let local = &name[sp.local_off as usize..];
    let mut tail = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if attr_matches_ns(doc, attr, ns, local) {
            doc.set_value_bytes(attr, val).map_err(|_| MutStatus::Oom)?;
            return Ok(attr);
        }
        tail = Some(attr);
        a = doc.next(attr);
    }
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        doc.store(ns).map_err(|_| MutStatus::Oom)?
    };
    build_attr(doc, el, name, &sp, val, nsv, tail)
}

/// Remove `el`'s attribute keyed by `(ns, local)`; `true` when one was removed.
pub fn remove_attribute_ns(doc: &mut Document, el: NodeId, ns: &[u8], local: &[u8]) -> bool {
    if doc.type_(el) != Some(NodeType::Element) {
        return false;
    }
    let mut prev: Option<NodeId> = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if attr_matches_ns(doc, attr, ns, local) {
            doc.unlink_attr(el, prev, attr);
            return true;
        }
        prev = Some(attr);
        a = doc.next(attr);
    }
    false
}

pub fn set_content(doc: &mut Document, node: NodeId, text: &[u8]) -> MutStatus {
    if !text.is_empty() && !validate_chars(text) {
        return MutStatus::BadChars;
    }
    match doc.type_(node) {
        Some(ty @ (NodeType::Text | NodeType::CData | NodeType::Comment | NodeType::Pi)) => {
            if !value_seq_ok(ty, text) {
                return MutStatus::BadChars;
            }
            if doc.set_value_bytes(node, text).is_err() {
                return MutStatus::Oom;
            }
            MutStatus::Ok
        }
        Some(NodeType::Element) => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: Option<NodeId> = None;
            if !text.is_empty() {
                let v = match doc.store(text) {
                    Ok(v) => v,
                    Err(_) => return MutStatus::Oom,
                };
                let n = match doc.new_node(NodeType::Text) {
                    Ok(n) => n,
                    Err(_) => return MutStatus::Oom,
                };
                doc.node_mut(n).value = v;
                t = Some(n);
            }
            let mut c = doc.first_child(node);
            while let Some(cur) = c {
                let nx = doc.next(cur);
                {
                    let n = doc.node_mut(cur);
                    n.parent = Link::NONE;
                    n.prev = Link::NONE;
                    n.next = Link::NONE;
                }
                c = nx;
            }
            {
                let n = doc.node_mut(node);
                n.first_child = Link::from_option(t);
                n.last_child = Link::from_option(t);
            }
            if let Some(t) = t {
                doc.set_parent(t, Some(node));
            }
            MutStatus::Ok
        }
        _ => MutStatus::Type,
    }
}

/* ============================ Phase 2: building ============================ */

pub fn new_element(doc: &mut Document, name: &[u8]) -> Result<NodeId, MutStatus> {
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutStatus::BadName),
    };
    if sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MutStatus::BadName); /* xmlns: is not an element prefix */
    }
    let el = doc
        .new_node(NodeType::Element)
        .map_err(|_| MutStatus::Oom)?;
    let st = assign_qname(doc, el, name, &sp);
    if st != MutStatus::Ok {
        return Err(st);
    }
    Ok(el) /* ns_uri stays unresolved until insertion */
}

/// A DOM-loose element: `name` may not be a valid XML QName (`":good:times:"`,
/// `"x<"`), so the caller supplies the prefix/local split explicitly and the
/// namespace URI directly.
pub fn new_loose_dom_element(
    doc: &mut Document,
    name: &[u8],
    sp: Split,
    ns: &[u8],
) -> Result<NodeId, MutStatus> {
    let Split {
        prefix_len,
        local_off,
        local_len,
    } = sp;
    if name.is_empty() || local_len == 0 {
        return Err(MutStatus::BadName);
    }
    if local_off as usize + local_len as usize > name.len() || prefix_len as usize > name.len() {
        return Err(MutStatus::BadName);
    }
    let el = doc
        .new_node(NodeType::Element)
        .map_err(|_| MutStatus::Oom)?;
    if doc
        .assign_qname(el, name, prefix_len, local_off, local_len)
        .is_err()
    {
        return Err(MutStatus::Oom);
    }
    if !ns.is_empty() {
        doc.set_ns_bytes(el, ns).map_err(|_| MutStatus::Oom)?;
    }
    doc.node_mut(el).flags |= FLAG_DOM_LOOSE_NAME;
    Ok(el)
}

pub fn new_chardata(doc: &mut Document, ty: NodeType, text: &[u8]) -> Result<NodeId, MutStatus> {
    if ty != NodeType::Text && ty != NodeType::CData && ty != NodeType::Comment {
        return Err(MutStatus::Type);
    }
    if !text.is_empty() && !validate_chars(text) {
        return Err(MutStatus::BadChars);
    }
    if !value_seq_ok(ty, text) {
        return Err(MutStatus::BadChars);
    }
    let n = doc.new_node(ty).map_err(|_| MutStatus::Oom)?;
    doc.set_value_bytes(n, text).map_err(|_| MutStatus::Oom)?;
    Ok(n)
}

pub fn new_pi(doc: &mut Document, target: &[u8], data: &[u8]) -> Result<NodeId, MutStatus> {
    if !crate::xml::chars::validate_name(target) || crate::xml::chars::is_reserved_pi_target(target)
    {
        return Err(MutStatus::BadName);
    }
    if !data.is_empty() && !validate_chars(data) {
        return Err(MutStatus::BadChars);
    }
    if !value_seq_ok(NodeType::Pi, data) {
        return Err(MutStatus::BadChars);
    }
    let pi = doc.new_node(NodeType::Pi).map_err(|_| MutStatus::Oom)?;
    let t = doc.store(target).map_err(|_| MutStatus::Oom)?;
    let d = doc.store(data).map_err(|_| MutStatus::Oom)?;
    {
        let n = doc.node_mut(pi);
        n.local = t;
        n.value = d;
    }
    Ok(pi)
}

pub fn new_document_type(
    doc: &mut Document,
    name: &[u8],
    pub_id: Option<&[u8]>,
    sys_id: Option<&[u8]>,
) -> Result<NodeId, MutStatus> {
    if !crate::xml::chars::validate_name(name) {
        return Err(MutStatus::BadName);
    }
    for id in [pub_id, sys_id].into_iter().flatten() {
        if !id.is_empty() && (!validate_chars(id) || id.contains(&b'"')) {
            return Err(MutStatus::BadChars);
        }
    }
    let dt = doc
        .new_node(NodeType::Doctype)
        .map_err(|_| MutStatus::Oom)?;
    let nm = doc.store(name).map_err(|_| MutStatus::Oom)?;
    {
        let n = doc.node_mut(dt);
        n.local = nm;
        n.qname = nm;
    }
    if let Some(p) = pub_id {
        let pp = doc.store(p).map_err(|_| MutStatus::Oom)?;
        doc.node_mut(dt).prefix = pp;
    }
    if let Some(s) = sys_id {
        let sp = doc.store(s).map_err(|_| MutStatus::Oom)?;
        doc.node_mut(dt).value = sp;
    }
    Ok(dt)
}

/// Resolve the namespace of element `e` and its attributes.
///
/// `commit` selects the pass: false only computes (to find out whether every
/// prefix in the subtree binds), true writes the resolved URIs.
fn resolve_node_ns(doc: &mut Document, e: NodeId, connected: bool, commit: bool) -> MutStatus {
    if doc.node(e).flags & FLAG_DOM_LOOSE_NAME == 0 {
        let name = match copy_span(doc.qname(e)) {
            Ok(v) => v,
            Err(st) => return st,
        };
        let sp = doc.split_of(e);
        match resolve_ns(doc, Some(e), &name, &sp, false, connected) {
            Ok(ns) => {
                if commit {
                    doc.node_mut(e).ns_uri = ns
                }
            }
            Err(st) => return st,
        }
    }
    let mut a = doc.attrs(e);
    while let Some(attr) = a {
        let name = match copy_span(doc.qname(attr)) {
            Ok(v) => v,
            Err(st) => return st,
        };
        let sp = doc.split_of(attr);
        match resolve_ns(doc, Some(e), &name, &sp, true, connected) {
            Ok(ns) => {
                if commit {
                    doc.node_mut(attr).ns_uri = ns
                }
            }
            Err(st) => return st,
        }
        a = doc.next(attr);
    }
    /* Only mark once connected: resolution inside a still-detached fragment is
     * deferred (an unbound prefix is not an error there), so the node must stay
     * open to being resolved again when the fragment joins the document. */
    if commit && connected {
        doc.node_mut(e).flags |= FLAG_NS_RESOLVED;
    }
    MutStatus::Ok
}

/// True once `e`'s namespace has been decided - by the parser, or by resolving
/// it against the context it was first inserted into.
fn ns_is_decided(doc: &Document, e: NodeId) -> bool {
    doc.node(e).flags & FLAG_NS_RESOLVED != 0
}

/// Re-resolve every element in `root`'s subtree, all-or-nothing: one pass that
/// only computes, and - only if every prefix binds - a second that writes.
fn resolve_subtree(doc: &mut Document, root: NodeId, connected: bool) -> MutStatus {
    for commit in [false, true] {
        let mut cur = Some(root);
        while let Some(c) = cur {
            if doc.type_(c) == Some(NodeType::Element) && !ns_is_decided(doc, c) {
                let st = resolve_node_ns(doc, c, connected, commit);
                if st != MutStatus::Ok {
                    return st; /* commit == false: nothing written yet */
                }
            }
            cur = doc.preorder_next(root, c);
        }
    }
    MutStatus::Ok
}

/// Resolve `node`'s subtree as if it were a child of `context`, WITHOUT linking
/// it (borrow node.parent for the ancestor walk, then restore).
fn resolve_into(doc: &mut Document, node: NodeId, context: NodeId) -> MutStatus {
    let saved = doc.parent(node);
    doc.set_parent(node, Some(context));
    let st = resolve_subtree(doc, node, doc.is_connected(node));
    doc.set_parent(node, saved);
    st
}

/* ---- copying ----
 *
 * A copy READS one arena and WRITES another - or, for `clone_node`, the same
 * one, which Rust cannot express as `(&mut Document, &Document)`. Rather than
 * keep two copies of every routine (one per source), a copy lifts the node's
 * fields OUT of the source first (`CopiedNode::read`) and writes them back
 * (`CopiedNode::write`). The fields had to be owned anyway - a `&mut Document`
 * cannot be held across a read of its own byte store - so the split costs
 * nothing and leaves one body per operation. */

/// A copied `value` span. XML distinguishes "never set" from "set to empty" - a
/// doctype's `PUBLIC ""` is present - so a copy has to carry the difference.
enum CopiedValue {
    Absent,
    Empty,
    Bytes(Vec<u8>),
}

/// One node's own fields, owned, out of any arena. Attributes come with it,
/// since they are part of the node's identity rather than its children.
struct CopiedNode {
    type_: NodeType,
    /// The qualified name and its split, for a node that has one.
    qname: Option<(Vec<u8>, Split)>,
    /// A bare local name (a PI target, a doctype name) on a node with no qname.
    local: Option<Vec<u8>>,
    value: CopiedValue,
    ns_uri: Option<Vec<u8>>,
    flags: u32,
    attrs: Vec<CopiedNode>,
}

/// The document a copy reads. `None` means the destination itself, which is
/// what a same-document [`clone_node`] needs: the caller re-borrows its
/// `&mut Document` as shared for each read, and this is the one line that says
/// so instead of a second copy of every routine.
type ReadFrom<'a> = Option<&'a Document>;

#[inline]
fn source<'a>(dst: &'a Document, from: ReadFrom<'a>) -> &'a Document {
    from.unwrap_or(dst)
}

impl CopiedNode {
    /// Lift `src`'s own fields and attributes out of `doc`.
    fn read(doc: &Document, src: NodeId) -> Result<CopiedNode, MutStatus> {
        let Some(node) = doc.try_node(src) else {
            return Err(MutStatus::Type);
        };
        let (type_, flags) = (node.type_, node.flags);
        let (qname_span, local_span, value_span, ns_span) =
            (node.qname, node.local, node.value, node.ns_uri);

        let qname = if qname_span.len > 0 {
            Some((copy_span(doc.qname(src))?, doc.split_of(src)))
        } else {
            None
        };
        let local = if qname_span.len == 0 && local_span.len > 0 {
            Some(copy_span(doc.local(src))?)
        } else {
            None
        };
        let value = if value_span.len > 0 {
            CopiedValue::Bytes(copy_span(doc.value(src))?)
        } else if value_span.is_absent() {
            CopiedValue::Absent
        } else {
            CopiedValue::Empty
        };
        let ns_uri = if ns_span.len > 0 {
            Some(copy_span(doc.ns(src))?)
        } else {
            None
        };

        let mut attrs: Vec<CopiedNode> = Vec::new();
        let mut a = doc.attrs(src);
        while let Some(attr) = a {
            attrs.falloc_reserve(1).map_err(|_| MutStatus::Oom)?;
            attrs.push(CopiedNode::read(doc, attr)?);
            a = doc.next(attr);
        }

        Ok(CopiedNode {
            type_,
            qname,
            local,
            value,
            ns_uri,
            flags,
            attrs,
        })
    }

    /// Write these fields as a fresh, detached node in `dst`.
    fn write(&self, dst: &mut Document) -> Result<NodeId, MutStatus> {
        let n = dst.new_node(self.type_).map_err(|_| MutStatus::Oom)?;
        if let Some((name, sp)) = &self.qname {
            if dst
                .assign_qname(n, name, sp.prefix_len, sp.local_off, sp.local_len)
                .is_err()
            {
                return Err(MutStatus::Oom);
            }
        } else if let Some(local) = &self.local {
            let span = dst.store(local).map_err(|_| MutStatus::Oom)?;
            dst.node_mut(n).local = span;
        }
        match &self.value {
            CopiedValue::Absent => {}
            CopiedValue::Empty => dst.node_mut(n).value = Span::EMPTY,
            CopiedValue::Bytes(v) => {
                let span = dst.store(v).map_err(|_| MutStatus::Oom)?;
                dst.node_mut(n).value = span;
            }
        }
        dst.node_mut(n).flags = self.flags;
        if let Some(uri) = &self.ns_uri {
            let span = dst.store(uri).map_err(|_| MutStatus::Oom)?;
            dst.node_mut(n).ns_uri = span;
        }
        /* attributes, in order */
        let mut tail: Option<NodeId> = None;
        for attr in &self.attrs {
            let ca = attr.write(dst)?;
            dst.link_attr(n, tail, ca);
            tail = Some(ca);
        }
        Ok(n)
    }
}

/// One arena copy of `src` - own fields and attributes, NOT children.
fn copy_one(dst: &mut Document, from: ReadFrom<'_>, src: NodeId) -> Result<NodeId, MutStatus> {
    let copied = CopiedNode::read(source(dst, from), src)?;
    copied.write(dst)
}

/// Deep copy of `src`'s subtree (iterative; no recursion, so a deep tree cannot
/// exhaust the stack).
fn deep_copy(dst: &mut Document, from: ReadFrom<'_>, src: NodeId) -> Result<NodeId, MutStatus> {
    let root = copy_one(dst, from, src)?;
    let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
    stack.falloc_reserve(1).map_err(|_| MutStatus::Oom)?;
    stack.push((src, root));
    while let Some((s, d)) = stack.pop() {
        let mut sc = source(dst, from).first_child(s);
        while let Some(child) = sc {
            let dc = copy_one(dst, from, child)?;
            dst.append_child(d, dc);
            if source(dst, from).first_child(child).is_some() {
                stack.falloc_reserve(1).map_err(|_| MutStatus::Oom)?;
                stack.push((child, dc));
            }
            sc = source(dst, from).next(child);
        }
    }
    Ok(root)
}

/// The cross-document entries take two distinct documents; a same-document copy
/// is [`clone_node`], which reads and writes one arena.
#[inline]
fn debug_assert_distinct(dst: &Document, src_doc: &Document) {
    debug_assert!(
        !core::ptr::eq(dst as *const Document, src_doc as *const Document),
        "same-document import must use clone_node, not the cross-document copy"
    );
}

/// Cross-document deep import (`importNode`).
pub fn import_subtree(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
) -> Result<NodeId, MutStatus> {
    debug_assert_distinct(dst, src_doc);
    deep_copy(dst, Some(src_doc), src)
}

/// Cross-document `copyNode`: shallow or deep, source in `src_doc`.
pub fn copy_node_from(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
    deep: bool,
) -> Result<NodeId, MutStatus> {
    debug_assert_distinct(dst, src_doc);
    if deep {
        deep_copy(dst, Some(src_doc), src)
    } else {
        copy_one(dst, Some(src_doc), src)
    }
}

/// Same-document `cloneNode`: shallow or deep, reading the arena it writes.
pub fn clone_node(doc: &mut Document, src: NodeId, deep: bool) -> Result<NodeId, MutStatus> {
    if deep {
        deep_copy(doc, None, src)
    } else {
        copy_one(doc, None, src)
    }
}

/* ---- insertion ---- */

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
    {
        let n = doc.node_mut(r);
        n.parent = Link::NONE;
        n.prev = Link::NONE;
        n.next = Link::NONE;
    }
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
    let st = check_fragment_children(doc, frag, Site::replacing(container, target));
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
