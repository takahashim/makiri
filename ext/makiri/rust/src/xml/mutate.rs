//! Mutation primitives (mkr_xml_mutate.c). Every primitive validates and
//! allocates BEFORE changing any link, so a failure leaves the tree untouched.
//!
//! The tree is an index arena, so this module is ordinary safe Rust under
//! `#![forbid(unsafe_code)]`: nodes are [`NodeId`] values, structure lives in
//! the [`Document`], and names/values are spans into its byte store.

#![forbid(unsafe_code)]

use crate::falloc::Reserve;
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, value_seq_ok, xmlns_prefix, Split};
use crate::xml::{
    Document, MutStatus, NodeId, NodeType, Span, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED,
};

/// A resolved namespace: a byte-store span (empty = no namespace).
type Ns = Span;

const NO_NS: Ns = Span::EMPTY;

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
        return Ok(match doc.resolve_in_scope(scope, b"") {
            Some(s) if s.len > 0 => s,
            _ => NO_NS,
        });
    }
    if prefix == b"xml" {
        return Ok(doc.xml_ns_span());
    }
    if prefix == b"xmlns" {
        return Err(MutStatus::BadName);
    }
    match doc.resolve_in_scope(scope, prefix) {
        Some(s) if s.len > 0 => Ok(s),
        _ => {
            if connected {
                Err(MutStatus::UnboundNs)
            } else {
                Ok(NO_NS)
            }
        }
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

pub fn detach(doc: &mut Document, node: NodeId) {
    doc.detach(node);
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

/// Build a fresh ATTRIBUTE (qname + value + namespace) and append it to `el`.
fn build_attr(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    sp: &Split,
    val: &[u8],
    ns: Ns,
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
    doc.append_attr(el, attr);
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
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if doc.qname(attr) == name {
            doc.set_value_bytes(attr, val).map_err(|_| MutStatus::Oom)?;
            doc.node_mut(attr).ns_uri = ns;
            return Ok(attr);
        }
        a = doc.next(attr);
    }
    build_attr(doc, el, name, &sp, val, ns)
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
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if attr_matches_ns(doc, attr, ns, local) {
            doc.set_value_bytes(attr, val).map_err(|_| MutStatus::Oom)?;
            return Ok(attr);
        }
        a = doc.next(attr);
    }
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        doc.store(ns).map_err(|_| MutStatus::Oom)?
    };
    build_attr(doc, el, name, &sp, val, nsv)
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
                    n.parent = None;
                    n.prev = None;
                    n.next = None;
                }
                c = nx;
            }
            {
                let n = doc.node_mut(node);
                n.first_child = t;
                n.last_child = t;
            }
            if let Some(t) = t {
                doc.node_mut(t).parent = Some(node);
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
    prefix_len: u32,
    local_off: u32,
    local_len: u32,
    ns: &[u8],
) -> Result<NodeId, MutStatus> {
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
        let name = doc.qname(e).to_vec();
        let prefix_len = doc.node(e).prefix.len;
        let local_off = doc.node(e).local.off.saturating_sub(doc.node(e).qname.off);
        let local_len = doc.node(e).local.len;
        let sp = Split {
            prefix_len,
            local_off,
            local_len,
        };
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
        let name = doc.qname(attr).to_vec();
        let prefix_len = doc.node(attr).prefix.len;
        let local_off = doc
            .node(attr)
            .local
            .off
            .saturating_sub(doc.node(attr).qname.off);
        let local_len = doc.node(attr).local.len;
        let sp = Split {
            prefix_len,
            local_off,
            local_len,
        };
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
    doc.node_mut(node).parent = Some(context);
    let st = resolve_subtree(doc, node, doc.is_connected(node));
    doc.node_mut(node).parent = saved;
    st
}

/// One arena copy of `src` (own fields + attributes, NOT children) from the
/// SAME document, INCLUDING its resolved namespace URI.
fn copy_one(doc: &mut Document, src: NodeId) -> Result<NodeId, MutStatus> {
    let Some(ty) = doc.type_(src) else {
        return Err(MutStatus::Type);
    };
    let n = doc.new_node(ty).map_err(|_| MutStatus::Oom)?;
    if doc.node(src).qname.len > 0 {
        let name = doc.qname(src).to_vec();
        let prefix_len = doc.node(src).prefix.len;
        let local_off = doc
            .node(src)
            .local
            .off
            .saturating_sub(doc.node(src).qname.off);
        let local_len = doc.node(src).local.len;
        if doc
            .assign_qname(n, &name, prefix_len, local_off, local_len)
            .is_err()
        {
            return Err(MutStatus::Oom);
        }
    } else if doc.node(src).local.len > 0 {
        let t = doc.local(src).to_vec();
        let span = doc.store(&t).map_err(|_| MutStatus::Oom)?;
        doc.node_mut(n).local = span;
    }
    if doc.node(src).value.len > 0 {
        let v = doc.value(src).to_vec();
        let span = doc.store(&v).map_err(|_| MutStatus::Oom)?;
        doc.node_mut(n).value = span;
    } else if !doc.node(src).value.is_absent() {
        doc.node_mut(n).value = Span::EMPTY;
    }
    doc.node_mut(n).flags = doc.node(src).flags;
    if doc.node(src).ns_uri.len > 0 {
        let u = doc.ns(src).to_vec();
        let span = doc.store(&u).map_err(|_| MutStatus::Oom)?;
        doc.node_mut(n).ns_uri = span;
    }
    /* copy attributes (each a node), preserving order */
    let mut tail: Option<NodeId> = None;
    let mut a = doc.attrs(src);
    while let Some(attr) = a {
        let ca = copy_one(doc, attr)?;
        doc.node_mut(ca).parent = Some(n);
        match tail {
            None => doc.node_mut(n).attrs = Some(ca),
            Some(t) => doc.node_mut(t).next = Some(ca),
        }
        tail = Some(ca);
        a = doc.next(attr);
    }
    Ok(n)
}

/// One arena copy of `src` from ANOTHER document (importNode's cross-kind
/// direction). Same fields as [`copy_one`], reading the source through its own
/// document and writing into `dst`.
fn copy_one_from(dst: &mut Document, src_doc: &Document, src: NodeId) -> Result<NodeId, MutStatus> {
    let Some(ty) = src_doc.type_(src) else {
        return Err(MutStatus::Type);
    };
    let n = dst.new_node(ty).map_err(|_| MutStatus::Oom)?;
    if src_doc.node(src).qname.len > 0 {
        let name = src_doc.qname(src).to_vec();
        let prefix_len = src_doc.node(src).prefix.len;
        let local_off = src_doc
            .node(src)
            .local
            .off
            .saturating_sub(src_doc.node(src).qname.off);
        let local_len = src_doc.node(src).local.len;
        if dst
            .assign_qname(n, &name, prefix_len, local_off, local_len)
            .is_err()
        {
            return Err(MutStatus::Oom);
        }
    } else if src_doc.node(src).local.len > 0 {
        let t = src_doc.local(src).to_vec();
        let span = dst.store(&t).map_err(|_| MutStatus::Oom)?;
        dst.node_mut(n).local = span;
    }
    if src_doc.node(src).value.len > 0 {
        let v = src_doc.value(src).to_vec();
        let span = dst.store(&v).map_err(|_| MutStatus::Oom)?;
        dst.node_mut(n).value = span;
    } else if !src_doc.node(src).value.is_absent() {
        dst.node_mut(n).value = Span::EMPTY;
    }
    dst.node_mut(n).flags = src_doc.node(src).flags;
    if src_doc.node(src).ns_uri.len > 0 {
        let u = src_doc.ns(src).to_vec();
        let span = dst.store(&u).map_err(|_| MutStatus::Oom)?;
        dst.node_mut(n).ns_uri = span;
    }
    let mut tail: Option<NodeId> = None;
    let mut a = src_doc.attrs(src);
    while let Some(attr) = a {
        let ca = copy_one_from(dst, src_doc, attr)?;
        dst.node_mut(ca).parent = Some(n);
        match tail {
            None => dst.node_mut(n).attrs = Some(ca),
            Some(t) => dst.node_mut(t).next = Some(ca),
        }
        tail = Some(ca);
        a = src_doc.next(attr);
    }
    Ok(n)
}

/// Deep copy of `src`'s subtree from ANOTHER document (iterative).
fn deep_copy_from(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
) -> Result<NodeId, MutStatus> {
    let root = copy_one_from(dst, src_doc, src)?;
    let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
    if stack.mkr_reserve(1).is_err() {
        return Err(MutStatus::Oom);
    }
    stack.push((src, root));
    while let Some((s, d)) = stack.pop() {
        let mut sc = src_doc.first_child(s);
        while let Some(child) = sc {
            let dc = copy_one_from(dst, src_doc, child)?;
            dst.append_child(d, dc);
            if src_doc.first_child(child).is_some() {
                if stack.mkr_reserve(1).is_err() {
                    return Err(MutStatus::Oom);
                }
                stack.push((child, dc));
            }
            sc = src_doc.next(child);
        }
    }
    Ok(root)
}

/// Deep copy of `src`'s subtree (iterative; no recursion).
fn deep_copy(doc: &mut Document, src: NodeId) -> Result<NodeId, MutStatus> {
    let root = copy_one(doc, src)?;
    let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
    if stack.mkr_reserve(1).is_err() {
        return Err(MutStatus::Oom);
    }
    stack.push((src, root));
    while let Some((s, d)) = stack.pop() {
        let mut sc = doc.first_child(s);
        while let Some(child) = sc {
            let dc = copy_one(doc, child)?;
            doc.append_child(d, dc);
            if doc.first_child(child).is_some() {
                if stack.mkr_reserve(1).is_err() {
                    return Err(MutStatus::Oom);
                }
                stack.push((child, dc));
            }
            sc = doc.next(child);
        }
    }
    Ok(root)
}

pub fn import_subtree(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
) -> Result<NodeId, MutStatus> {
    deep_copy_from(dst, src_doc, src)
}

/// Cross-document `copyNode`: shallow or deep, source in `src_doc`.
pub fn copy_node_from(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
    deep: bool,
) -> Result<NodeId, MutStatus> {
    if deep {
        deep_copy_from(dst, src_doc, src)
    } else {
        copy_one_from(dst, src_doc, src)
    }
}

pub fn clone_node(doc: &mut Document, src: NodeId, deep: bool) -> Result<NodeId, MutStatus> {
    if deep {
        deep_copy(doc, src)
    } else {
        copy_one(doc, src)
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

/// WHATWG doctype ordering at the document node (fail-closed).
fn check_doc_child_order(
    doc: &Document,
    container: NodeId,
    node: NodeId,
    before: Option<NodeId>,
    exclude: Option<NodeId>,
) -> MutStatus {
    if doc.type_(container) != Some(NodeType::Document) {
        return if doc.type_(node) == Some(NodeType::Doctype) {
            MutStatus::Hierarchy
        } else {
            MutStatus::Ok
        };
    }
    if doc.type_(node) == Some(NodeType::Doctype) {
        let mut c = doc.first_child(container);
        while let Some(cur) = c {
            if Some(cur) != exclude && cur != node && doc.type_(cur) == Some(NodeType::Doctype) {
                return MutStatus::Hierarchy; /* at most one */
            }
            c = doc.next(cur);
        }
        /* no element before the doctype */
        let mut c = doc.first_child(container);
        while let Some(cur) = c {
            if c == before {
                break;
            }
            if Some(cur) != exclude && cur != node && doc.type_(cur) == Some(NodeType::Element) {
                return MutStatus::Hierarchy;
            }
            c = doc.next(cur);
        }
        return MutStatus::Ok;
    }
    if doc.type_(node) == Some(NodeType::Element) {
        let mut c = before;
        while let Some(cur) = c {
            if Some(cur) != exclude && cur != node && doc.type_(cur) == Some(NodeType::Doctype) {
                return MutStatus::Hierarchy;
            }
            c = doc.next(cur);
        }
    }
    MutStatus::Ok
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

fn doc_root_ok(doc: &Document, container: NodeId, node: NodeId, exclude: Option<NodeId>) -> bool {
    if doc.type_(container) != Some(NodeType::Document)
        || doc.type_(node) != Some(NodeType::Element)
    {
        return true;
    }
    let mut c = doc.first_child(container);
    while let Some(cur) = c {
        if Some(cur) != exclude && cur != node && doc.type_(cur) == Some(NodeType::Element) {
            return false;
        }
        c = doc.next(cur);
    }
    true
}

/// Validation + namespace resolution for inserting `node` under `container`
/// before `before` (None = append), replacing `exclude` (or None). No
/// structural change.
fn prepare_insert(
    doc: &mut Document,
    container: NodeId,
    node: NodeId,
    before: Option<NodeId>,
    exclude: Option<NodeId>,
) -> MutStatus {
    if !is_insertable(doc, node) {
        return MutStatus::Hierarchy;
    }
    let ct = doc.type_(container);
    if ct != Some(NodeType::Element) && ct != Some(NodeType::Document) {
        return MutStatus::Hierarchy;
    }
    if would_cycle(doc, container, node) {
        return MutStatus::Cycle;
    }
    if !doc_root_ok(doc, container, node, exclude) {
        return MutStatus::Hierarchy;
    }
    let dt = check_doc_child_order(doc, container, node, before, exclude);
    if dt != MutStatus::Ok {
        return dt;
    }
    resolve_into(doc, node, container)
}

pub fn insert_child(doc: &mut Document, parent: NodeId, node: NodeId) -> MutStatus {
    let st = prepare_insert(doc, parent, node, None, None);
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
    let st = prepare_insert(doc, container, node, Some(r), None);
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
    let st = prepare_insert(doc, container, node, next, None);
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
    let st = prepare_insert(doc, container, node, Some(r), Some(r));
    if st != MutStatus::Ok {
        return st;
    }
    doc.detach(node);
    let (prev, next) = (doc.prev(r), doc.next(r));
    doc.splice_between(container, node, prev, next);
    {
        let n = doc.node_mut(r);
        n.parent = None;
        n.prev = None;
        n.next = None;
    }
    doc.sync_doc_meta(container);
    MutStatus::Ok
}

pub fn remove(doc: &mut Document, node: NodeId) {
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
    if doc.type_(container) == Some(NodeType::Document) {
        if element_child_count(doc, frag, None) + element_child_count(doc, container, Some(target))
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
    }
    let mut c = doc.first_child(frag);
    while let Some(cur) = c {
        let st = prepare_insert(doc, container, cur, Some(target), Some(target));
        if st != MutStatus::Ok {
            return st;
        }
        c = doc.next(cur);
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
