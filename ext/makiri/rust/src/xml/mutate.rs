//! Mutation primitives (mkr_xml_mutate.c). Every primitive validates and
//! allocates BEFORE changing any link, so a failure leaves the tree untouched.
//!
//! This module composes the raw pointer-linking in [`crate::xml::raw`] and the
//! arena allocation in [`crate::xml::arena`]; it holds no raw-pointer
//! dereference of its own and is therefore ordinary Rust under
//! `#![forbid(unsafe_code)]`. `ffi.rs` is the only place a raw `*mut Node`
//! becomes a [`NodeRef`].
//!
//! Invariants the API carries:
//!
//! - a [`NodeRef`] is non-NULL and named by a live document arena;
//! - insertion only ever links nodes owned by the target document (the caller
//!   passes the arena, and factories allocate from it);
//! - self-cycles and document/attribute placement are rejected by
//!   `prepare_insert`, so a rejected move changes nothing;
//! - allocation precedes relinking, so an OOM leaves the tree as it was.

#![forbid(unsafe_code)]

use crate::falloc::Reserve;
use crate::xml::arena::Arena;
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, value_seq_ok, xmlns_prefix};
use crate::xml::raw::{self, NodeRef};
use crate::xml::{
    empty, qname_from, QName, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED, MUT_BAD_CHARS, MUT_BAD_NAME,
    MUT_BAD_NS_DECL, MUT_CYCLE, MUT_HIERARCHY, MUT_OK, MUT_OOM, MUT_TYPE, MUT_UNBOUND_NS,
    T_ATTRIBUTE, T_CDATA, T_COMMENT, T_DOCTYPE, T_DOCUMENT, T_ELEMENT, T_PI, T_TEXT, XMLNS_NS_URI,
    XML_NS_URI,
};
use core::ffi::c_char;
use core::ptr;

type Ns = (*const c_char, u32);

const NO_NS: Ns = (ptr::null(), 0);

#[inline]
fn xml_ns() -> Ns {
    (
        XML_NS_URI.as_ptr() as *const c_char,
        XML_NS_URI.len() as u32,
    )
}
#[inline]
fn xmlns_ns() -> Ns {
    (
        XMLNS_NS_URI.as_ptr() as *const c_char,
        XMLNS_NS_URI.len() as u32,
    )
}

/// Resolve `qn` applied at `scope` (mirrors the parser's §7 rules). An unbound
/// prefix is an error only when connected; deferred (unresolved) otherwise.
fn resolve_ns(
    scope: Option<NodeRef>,
    qn: &QName,
    is_attr: bool,
    connected: bool,
) -> Result<Ns, i32> {
    let qname = raw::bytes(qn.qname, qn.qname_len);
    let prefix = raw::bytes(qn.prefix, qn.prefix_len);
    if is_attr && xmlns_prefix(qname).is_some() {
        return Ok(xmlns_ns());
    }
    if qn.prefix_len == 0 {
        if is_attr {
            return Ok(NO_NS); /* unprefixed attribute -> no namespace */
        }
        return Ok(match raw::resolve_in_scope(scope, b"") {
            Some((u, ul)) if ul > 0 => (u, ul),
            _ => NO_NS,
        });
    }
    if prefix == b"xml" {
        return Ok(xml_ns());
    }
    if prefix == b"xmlns" {
        return Err(MUT_BAD_NAME);
    }
    match raw::resolve_in_scope(scope, prefix) {
        Some((u, ul)) if ul > 0 => Ok((u, ul)),
        _ => {
            if connected {
                Err(MUT_UNBOUND_NS)
            } else {
                Ok(NO_NS)
            }
        }
    }
}

#[inline]
fn assign_qname(doc: Arena, node: NodeRef, qn: &QName) -> i32 {
    if doc.assign_qname(node.as_ptr(), qn) == 0 {
        MUT_OK
    } else {
        MUT_OOM
    }
}

#[inline]
fn set_ns(n: NodeRef, ns: Ns) {
    n.set_ns(ns.0, ns.1);
}

pub fn detach(node: NodeRef) {
    raw::detach(node);
}

pub fn rename(doc: Arena, node: NodeRef, name: &[u8]) -> i32 {
    if node.type_() != T_ELEMENT && node.type_() != T_ATTRIBUTE {
        return MUT_TYPE;
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MUT_BAD_NAME,
    };
    let qn = qname_from(name, &sp);
    let is_attr = node.type_() == T_ATTRIBUTE;
    let scope: Option<NodeRef> = if is_attr { node.parent() } else { Some(node) };
    let connected = scope.is_some_and(raw::is_connected);
    let ns = match resolve_ns(scope, &qn, is_attr, connected) {
        Ok(ns) => ns,
        Err(st) => return st,
    };
    /* copy the new qname BEFORE writing ns_uri, so an OOM leaves node intact */
    let st = assign_qname(doc, node, &qn);
    if st != MUT_OK {
        return st;
    }
    set_ns(node, ns);
    node.clear_flag(FLAG_DOM_LOOSE_NAME);
    /* A rename picks a new prefix, so it decides a new URI from the scope the
     * node is in right now - and that decision is the node's identity from here
     * (an element's; an attribute follows its element). */
    if connected && !is_attr {
        node.add_flag(FLAG_NS_RESOLVED);
    }
    MUT_OK
}

/// Build a fresh ATTRIBUTE (qname + value + namespace) and append it to `el`.
fn build_attr(doc: Arena, el: NodeRef, qn: &QName, val: &[u8], ns: Ns) -> Result<NodeRef, i32> {
    let attr = doc.alloc_node(T_ATTRIBUTE).ok_or(MUT_OOM)?;
    let st = assign_qname(doc, attr, qn);
    if st != MUT_OK {
        return Err(st);
    }
    let nv = doc.bytes(val);
    if nv.is_null() {
        return Err(MUT_OOM);
    }
    attr.set_value(nv, val.len() as u32);
    set_ns(attr, ns);
    raw::append_attr(el, attr);
    Ok(attr)
}

pub fn set_attribute(doc: Arena, el: NodeRef, name: &[u8], val: &[u8]) -> Result<NodeRef, i32> {
    if el.type_() != T_ELEMENT {
        return Err(MUT_TYPE);
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MUT_BAD_NAME),
    };
    let qn = qname_from(name, &sp);
    /* xmlns:foo="" must not bind a prefix to the empty namespace */
    if val.is_empty() && sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MUT_BAD_NS_DECL);
    }
    if !val.is_empty() && !validate_chars(val) {
        return Err(MUT_BAD_CHARS);
    }
    let ns = resolve_ns(Some(el), &qn, true, raw::is_connected(el))?;
    /* an existing attribute with the same raw QName -> replace its value */
    let mut a = el.attrs();
    while let Some(attr) = a {
        if attr.qname() == name {
            let nv = doc.bytes(val);
            if nv.is_null() {
                return Err(MUT_OOM);
            }
            attr.set_value(nv, val.len() as u32);
            set_ns(attr, ns);
            return Ok(attr);
        }
        a = attr.next();
    }
    build_attr(doc, el, &qn, val, ns)
}

pub fn remove_attribute(el: NodeRef, name: &[u8]) -> i32 {
    if el.type_() != T_ELEMENT {
        return 0;
    }
    let mut prev: Option<NodeRef> = None;
    let mut a = el.attrs();
    while let Some(attr) = a {
        if attr.qname() == name {
            raw::unlink_attr(el, prev, attr);
            return 1;
        }
        prev = Some(attr);
        a = attr.next();
    }
    0
}

/// `a` is keyed by (ns, local) - the DOM key; an empty wanted namespace
/// matches an attribute with no namespace.
fn attr_matches_ns(a: NodeRef, ns: &[u8], local: &[u8]) -> bool {
    a.ns_uri_len() as usize == ns.len() && (ns.is_empty() || a.ns() == ns) && a.local() == local
}

pub fn set_attribute_ns(
    doc: Arena,
    el: NodeRef,
    ns: &[u8],
    name: &[u8],
    val: &[u8],
) -> Result<NodeRef, i32> {
    if el.type_() != T_ELEMENT {
        return Err(MUT_TYPE);
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MUT_BAD_NAME),
    };
    let qn = qname_from(name, &sp);
    if !val.is_empty() && !validate_chars(val) {
        return Err(MUT_BAD_CHARS);
    }
    let local = &name[sp.local_off as usize..];
    let mut a = el.attrs();
    while let Some(attr) = a {
        if attr_matches_ns(attr, ns, local) {
            let nv = doc.bytes(val);
            if nv.is_null() {
                return Err(MUT_OOM);
            }
            attr.set_value(nv, val.len() as u32);
            return Ok(attr);
        }
        a = attr.next();
    }
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        let u = doc.bytes(ns);
        if u.is_null() {
            return Err(MUT_OOM);
        }
        (u, ns.len() as u32)
    };
    build_attr(doc, el, &qn, val, nsv)
}

pub fn remove_attribute_ns(el: NodeRef, ns: &[u8], local: &[u8]) -> i32 {
    if el.type_() != T_ELEMENT {
        return 0;
    }
    let mut prev: Option<NodeRef> = None;
    let mut a = el.attrs();
    while let Some(attr) = a {
        if attr_matches_ns(attr, ns, local) {
            raw::unlink_attr(el, prev, attr);
            return 1;
        }
        prev = Some(attr);
        a = attr.next();
    }
    0
}

pub fn set_content(doc: Arena, node: NodeRef, text: &[u8]) -> i32 {
    if !text.is_empty() && !validate_chars(text) {
        return MUT_BAD_CHARS;
    }
    match node.type_() {
        T_TEXT | T_CDATA | T_COMMENT | T_PI => {
            if !value_seq_ok(node.type_(), text) {
                return MUT_BAD_CHARS;
            }
            let nv = doc.bytes(text);
            if nv.is_null() {
                return MUT_OOM;
            }
            node.set_value(nv, text.len() as u32);
            MUT_OK
        }
        T_ELEMENT => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: Option<NodeRef> = None;
            if !text.is_empty() {
                let nv = doc.bytes(text);
                if nv.is_null() {
                    return MUT_OOM;
                }
                let n = match doc.alloc_node(T_TEXT) {
                    Some(n) => n,
                    None => return MUT_OOM,
                };
                n.set_value(nv, text.len() as u32);
                t = Some(n);
            }
            let mut c = node.first_child();
            while let Some(cur) = c {
                let nx = cur.next();
                cur.clear_links();
                c = nx;
            }
            node.set_first_child(t);
            node.set_last_child(t);
            if let Some(t) = t {
                t.set_parent(Some(node));
            }
            MUT_OK
        }
        _ => MUT_TYPE,
    }
}

/* ============================ Phase 2: building ============================ */

pub fn new_element(doc: Arena, name: &[u8]) -> Result<NodeRef, i32> {
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MUT_BAD_NAME),
    };
    if sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MUT_BAD_NAME); /* xmlns: is not an element prefix */
    }
    let el = doc.alloc_node(T_ELEMENT).ok_or(MUT_OOM)?;
    let st = assign_qname(doc, el, &qname_from(name, &sp));
    if st != MUT_OK {
        return Err(st);
    }
    Ok(el) /* ns_uri stays unresolved until insertion */
}

pub fn new_loose_dom_element(doc: Arena, qn: &QName, ns: &[u8]) -> Result<NodeRef, i32> {
    if qn.qname_len == 0 || qn.local_len == 0 {
        return Err(MUT_BAD_NAME);
    }
    let (q0, l0) = (qn.qname as usize, qn.local as usize);
    if !(l0 >= q0
        && qn.local_len <= qn.qname_len
        && (l0 - q0) <= (qn.qname_len - qn.local_len) as usize)
    {
        return Err(MUT_BAD_NAME);
    }
    let el = doc.alloc_node(T_ELEMENT).ok_or(MUT_OOM)?;
    let st = assign_qname(doc, el, qn);
    if st != MUT_OK {
        return Err(st);
    }
    if !ns.is_empty() {
        let u = doc.bytes(ns);
        if u.is_null() {
            return Err(MUT_OOM);
        }
        set_ns(el, (u, ns.len() as u32));
    }
    el.add_flag(FLAG_DOM_LOOSE_NAME);
    Ok(el)
}

pub fn new_chardata(doc: Arena, ty: u32, text: &[u8]) -> Result<NodeRef, i32> {
    if ty != T_TEXT && ty != T_CDATA && ty != T_COMMENT {
        return Err(MUT_TYPE);
    }
    if !text.is_empty() && !validate_chars(text) {
        return Err(MUT_BAD_CHARS);
    }
    if !value_seq_ok(ty, text) {
        return Err(MUT_BAD_CHARS);
    }
    let n = doc.alloc_node(ty).ok_or(MUT_OOM)?;
    let v = doc.bytes(text);
    if v.is_null() {
        return Err(MUT_OOM);
    }
    n.set_value(v, text.len() as u32);
    Ok(n)
}

pub fn new_pi(doc: Arena, target: &[u8], data: &[u8]) -> Result<NodeRef, i32> {
    if !crate::xml::chars::validate_name(target) || crate::xml::chars::is_reserved_pi_target(target)
    {
        return Err(MUT_BAD_NAME);
    }
    if !data.is_empty() && !validate_chars(data) {
        return Err(MUT_BAD_CHARS);
    }
    if !value_seq_ok(T_PI, data) {
        return Err(MUT_BAD_CHARS);
    }
    let pi = doc.alloc_node(T_PI).ok_or(MUT_OOM)?;
    let t = doc.bytes(target);
    if t.is_null() {
        return Err(MUT_OOM);
    }
    let d = doc.bytes(data);
    if d.is_null() {
        return Err(MUT_OOM);
    }
    pi.set_local(t, target.len() as u32);
    pi.set_value(d, data.len() as u32);
    Ok(pi)
}

pub fn new_document_type(
    doc: Arena,
    name: &[u8],
    pub_id: Option<&[u8]>,
    sys_id: Option<&[u8]>,
) -> Result<NodeRef, i32> {
    if !crate::xml::chars::validate_name(name) {
        return Err(MUT_BAD_NAME);
    }
    for id in [pub_id, sys_id].into_iter().flatten() {
        if !id.is_empty() && (!validate_chars(id) || id.contains(&b'"')) {
            return Err(MUT_BAD_CHARS);
        }
    }
    let dt = doc.alloc_node(T_DOCTYPE).ok_or(MUT_OOM)?;
    let nm = doc.bytes(name);
    if nm.is_null() {
        return Err(MUT_OOM);
    }
    dt.set_local(nm, name.len() as u32);
    dt.set_qname_parts(nm, name.len() as u32);
    if let Some(p) = pub_id {
        let pp = doc.bytes(p);
        if pp.is_null() {
            return Err(MUT_OOM);
        }
        dt.set_prefix(pp, p.len() as u32);
    }
    if let Some(s) = sys_id {
        let sp = doc.bytes(s);
        if sp.is_null() {
            return Err(MUT_OOM);
        }
        dt.set_value(sp, s.len() as u32);
    }
    Ok(dt)
}

/// Resolve the namespace of element `e` and its attributes.
///
/// `commit` selects the pass: false only computes (to find out whether every
/// prefix in the subtree binds), true writes the resolved URIs. See
/// `resolve_subtree`.
fn resolve_node_ns(e: NodeRef, connected: bool, commit: bool) -> i32 {
    if e.flags() & FLAG_DOM_LOOSE_NAME == 0 {
        let eq = e.qname_of();
        match resolve_ns(Some(e), &eq, false, connected) {
            Ok(ns) => {
                if commit {
                    set_ns(e, ns)
                }
            }
            Err(st) => return st,
        }
    }
    let mut a = e.attrs();
    while let Some(attr) = a {
        let aq = attr.qname_of();
        match resolve_ns(Some(e), &aq, true, connected) {
            Ok(ns) => {
                if commit {
                    set_ns(attr, ns)
                }
            }
            Err(st) => return st,
        }
        a = attr.next();
    }
    /* Only mark once connected: resolution inside a still-detached fragment is
     * deferred (an unbound prefix is not an error there), so the node must stay
     * open to being resolved again when the fragment joins the document. */
    if commit && connected {
        e.add_flag(FLAG_NS_RESOLVED);
    }
    MUT_OK
}

/// True once `e`'s namespace has been decided - by the parser, or by resolving
/// it against the context it was first inserted into. From then on the URI is
/// the node's identity, so a later move must NOT re-derive it: that is what
/// makes namespaceURI survive a move the way the DOM and browsers have it, and
/// the serializer emits whatever declarations the output needs.
fn ns_is_decided(e: NodeRef) -> bool {
    e.flags() & FLAG_NS_RESOLVED != 0
}

/// Re-resolve every element in `root`'s subtree, all-or-nothing.
///
/// The walk writes as it goes, so a bare single pass that fails partway leaves
/// the subtree half-rewritten: the elements before the unbound prefix carry URIs
/// resolved against a scope the tree is not in, while the rest keep the old
/// ones. That state is invisible to serialization (only prefixes are written)
/// but wrong for XPath, which matches on the resolved URI. So: one pass that
/// only computes, and - only if every prefix binds - a second that writes.
fn resolve_subtree(root: NodeRef, connected: bool) -> i32 {
    /* Both passes run the SAME body - that is the point of the loop rather than
     * two functions. If the check could drift from the commit, that drift would
     * be the bug. */
    for commit in [false, true] {
        let mut cur = Some(root);
        while let Some(c) = cur {
            if c.type_() == T_ELEMENT && !ns_is_decided(c) {
                let st = resolve_node_ns(c, connected, commit);
                if st != MUT_OK {
                    return st; /* commit == false: nothing written yet */
                }
            }
            cur = raw::preorder_next(root, c);
        }
    }
    MUT_OK
}

/// Resolve `node`'s subtree as if it were a child of `context`, WITHOUT
/// linking it (borrow node.parent for the ancestor walk, then restore).
fn resolve_into(node: NodeRef, context: NodeRef) -> i32 {
    let saved = node.parent();
    node.set_parent(Some(context));
    let st = resolve_subtree(node, raw::is_connected(node));
    node.set_parent(saved);
    st
}

/// One arena copy of `src` (own fields + attributes, NOT children), INCLUDING
/// its resolved namespace URI. A copy keeps the namespace it had: the URI is the
/// node identity, so neither cloneNode nor importNode re-derives it from wherever
/// the copy lands - what the DOM and browsers do.
fn copy_one(doc: Arena, src: NodeRef) -> Result<NodeRef, i32> {
    let n = doc.alloc_node(src.type_()).ok_or(MUT_OOM)?;
    if !src.qname_ptr().is_null() && src.qname_len() > 0 {
        let qn = src.qname_of();
        if assign_qname(doc, n, &qn) != MUT_OK {
            return Err(MUT_OOM);
        }
    } else if !src.local().is_empty() {
        let t = doc.bytes(src.local()); /* PI target */
        if t.is_null() {
            return Err(MUT_OOM);
        }
        n.set_local(t, src.local_len());
    }
    if src.value_len() > 0 {
        let v = doc.bytes(src.value());
        if v.is_null() {
            return Err(MUT_OOM);
        }
        n.set_value(v, src.value_len());
    } else if !src.value_ptr().is_null() {
        n.set_value(empty(), 0);
    }
    n.set_flags(src.flags());
    if !src.ns_uri_ptr().is_null() && src.ns_uri_len() > 0 {
        let u = doc.bytes(src.ns());
        if u.is_null() {
            return Err(MUT_OOM);
        }
        n.set_ns(u, src.ns_uri_len());
    }
    /* copy attributes (each an arena node), preserving order */
    let mut tail: Option<NodeRef> = None;
    let mut a = src.attrs();
    while let Some(attr) = a {
        let ca = copy_one(doc, attr)?; /* an attribute has no children/attrs */
        ca.set_parent(Some(n));
        match tail {
            None => n.set_attrs(Some(ca)),
            Some(t) => t.set_next(Some(ca)),
        }
        tail = Some(ca);
        a = attr.next();
    }
    Ok(n)
}

/// Deep copy of `src`'s subtree (iterative; no recursion).
fn deep_copy(doc: Arena, src: NodeRef) -> Result<NodeRef, i32> {
    let root = copy_one(doc, src)?;
    let mut stack: Vec<(NodeRef, NodeRef)> = Vec::new();
    if stack.mkr_reserve(1).is_err() {
        return Err(MUT_OOM);
    }
    stack.push((src, root));
    while let Some((s, d)) = stack.pop() {
        let mut sc = s.first_child();
        while let Some(child) = sc {
            let dc = copy_one(doc, child)?;
            raw::append_child(d, dc);
            if child.first_child().is_some() {
                if stack.mkr_reserve(1).is_err() {
                    return Err(MUT_OOM);
                }
                stack.push((child, dc));
            }
            sc = child.next();
        }
    }
    Ok(root)
}

pub fn import_subtree(doc: Arena, src: NodeRef) -> Result<NodeRef, i32> {
    deep_copy(doc, src)
}

pub fn clone_node(doc: Arena, src: NodeRef, deep: bool) -> Result<NodeRef, i32> {
    if deep {
        deep_copy(doc, src)
    } else {
        copy_one(doc, src)
    }
}

pub fn copy_node(doc: Arena, src: NodeRef, deep: bool) -> Result<NodeRef, i32> {
    if deep {
        deep_copy(doc, src)
    } else {
        copy_one(doc, src)
    }
}

/* ---- insertion ---- */

#[inline]
fn is_insertable(node: NodeRef) -> bool {
    matches!(
        node.type_(),
        T_ELEMENT | T_TEXT | T_CDATA | T_COMMENT | T_PI | T_DOCTYPE
    )
}

/// WHATWG doctype ordering at the document node (fail-closed).
fn check_doc_child_order(
    container: NodeRef,
    node: NodeRef,
    before: Option<NodeRef>,
    exclude: Option<NodeRef>,
) -> i32 {
    if container.type_() != T_DOCUMENT {
        return if node.type_() == T_DOCTYPE {
            MUT_HIERARCHY
        } else {
            MUT_OK
        };
    }
    if node.type_() == T_DOCTYPE {
        let mut c = container.first_child();
        while let Some(cur) = c {
            if Some(cur) != exclude && cur != node && cur.type_() == T_DOCTYPE {
                return MUT_HIERARCHY; /* at most one */
            }
            c = cur.next();
        }
        /* no element before the doctype */
        let mut c = container.first_child();
        while let Some(cur) = c {
            if c == before {
                break;
            }
            if Some(cur) != exclude && cur != node && cur.type_() == T_ELEMENT {
                return MUT_HIERARCHY;
            }
            c = cur.next();
        }
        return MUT_OK;
    }
    if node.type_() == T_ELEMENT {
        let mut c = before;
        while let Some(cur) = c {
            if Some(cur) != exclude && cur != node && cur.type_() == T_DOCTYPE {
                return MUT_HIERARCHY;
            }
            c = cur.next();
        }
    }
    MUT_OK
}

fn would_cycle(container: NodeRef, node: NodeRef) -> bool {
    let mut p = Some(container);
    while let Some(cur) = p {
        if cur == node {
            return true;
        }
        p = cur.parent();
    }
    false
}

fn doc_root_ok(container: NodeRef, node: NodeRef, exclude: Option<NodeRef>) -> bool {
    if container.type_() != T_DOCUMENT || node.type_() != T_ELEMENT {
        return true;
    }
    let mut c = container.first_child();
    while let Some(cur) = c {
        if Some(cur) != exclude && cur != node && cur.type_() == T_ELEMENT {
            return false;
        }
        c = cur.next();
    }
    true
}

/// Re-derive doc.root / doc.doctype from the tree after a change at the
/// document node.
fn sync_doc_meta(doc: Arena, container: NodeRef) {
    if !ptr::eq(container.as_ptr(), doc.document_node()) {
        return;
    }
    doc.set_root(ptr::null_mut());
    doc.set_doctype(ptr::null_mut());
    let Some(dn) = doc.doc_node_ref() else {
        return;
    };
    let mut c = dn.first_child();
    while let Some(cur) = c {
        if doc.root().is_null() && cur.type_() == T_ELEMENT {
            doc.set_root(cur.as_ptr());
        }
        if doc.doctype().is_null() && cur.type_() == T_DOCTYPE {
            doc.set_doctype(cur.as_ptr());
        }
        c = cur.next();
    }
}

/// Validation + namespace resolution for inserting `node` under `container`
/// before `before` (None = append), replacing `exclude` (or None). No
/// structural change.
fn prepare_insert(
    container: NodeRef,
    node: NodeRef,
    before: Option<NodeRef>,
    exclude: Option<NodeRef>,
) -> i32 {
    if !is_insertable(node) {
        return MUT_HIERARCHY;
    }
    let ct = container.type_();
    if ct != T_ELEMENT && ct != T_DOCUMENT {
        return MUT_HIERARCHY;
    }
    if would_cycle(container, node) {
        return MUT_CYCLE;
    }
    if !doc_root_ok(container, node, exclude) {
        return MUT_HIERARCHY;
    }
    let dt = check_doc_child_order(container, node, before, exclude);
    if dt != MUT_OK {
        return dt;
    }
    resolve_into(node, container)
}

pub fn insert_child(doc: Arena, parent: NodeRef, node: NodeRef) -> i32 {
    let st = prepare_insert(parent, node, None, None);
    if st != MUT_OK {
        return st;
    }
    raw::detach(node);
    raw::splice_between(parent, node, parent.last_child(), None);
    sync_doc_meta(doc, parent);
    MUT_OK
}

pub fn insert_before(doc: Arena, r: NodeRef, node: NodeRef) -> i32 {
    if node == r {
        return MUT_OK;
    }
    let Some(container) = r.parent() else {
        return MUT_HIERARCHY;
    };
    let st = prepare_insert(container, node, Some(r), None);
    if st != MUT_OK {
        return st;
    }
    raw::detach(node);
    raw::splice_between(container, node, r.prev(), Some(r));
    sync_doc_meta(doc, container);
    MUT_OK
}

pub fn insert_after(doc: Arena, r: NodeRef, node: NodeRef) -> i32 {
    if node == r {
        return MUT_OK;
    }
    let Some(container) = r.parent() else {
        return MUT_HIERARCHY;
    };
    let st = prepare_insert(container, node, r.next(), None);
    if st != MUT_OK {
        return st;
    }
    raw::detach(node);
    raw::splice_between(container, node, Some(r), r.next());
    sync_doc_meta(doc, container);
    MUT_OK
}

pub fn replace_node(doc: Arena, r: NodeRef, node: NodeRef) -> i32 {
    let Some(container) = r.parent() else {
        return MUT_HIERARCHY;
    };
    if node == r {
        return MUT_OK;
    }
    let st = prepare_insert(container, node, Some(r), Some(r));
    if st != MUT_OK {
        return st;
    }
    raw::detach(node);
    raw::splice_between(container, node, r.prev(), r.next());
    r.clear_links();
    sync_doc_meta(doc, container);
    MUT_OK
}

pub fn remove(doc: Arena, node: NodeRef) {
    let parent = node.parent();
    raw::detach(node);
    if let Some(p) = parent {
        sync_doc_meta(doc, p);
    }
}

fn element_child_count(parent: NodeRef, exclude: Option<NodeRef>) -> usize {
    let mut n = 0;
    let mut c = parent.first_child();
    while let Some(cur) = c {
        if Some(cur) != exclude && cur.type_() == T_ELEMENT {
            n += 1;
        }
        c = cur.next();
    }
    n
}

/// Replace `target` with the CHILDREN of `frag`, atomically (fail-closed).
pub fn replace_with_fragment(doc: Arena, target: NodeRef, frag: NodeRef) -> i32 {
    let Some(container) = target.parent() else {
        return MUT_HIERARCHY;
    };
    /* --- validation pass: no links change until it all passes */
    if container.type_() == T_DOCUMENT {
        if element_child_count(frag, None) + element_child_count(container, Some(target)) > 1 {
            return MUT_HIERARCHY;
        }
        let mut c = frag.first_child();
        while let Some(cur) = c {
            if cur.type_() == T_DOCTYPE {
                return MUT_HIERARCHY;
            }
            c = cur.next();
        }
    }
    let mut c = frag.first_child();
    while let Some(cur) = c {
        let st = prepare_insert(container, cur, Some(target), Some(target));
        if st != MUT_OK {
            return st;
        }
        c = cur.next();
    }
    /* --- commit pass: every child takes target's slot in fragment order */
    while let Some(c) = frag.first_child() {
        raw::detach(c);
        raw::splice_between(container, c, target.prev(), Some(target));
    }
    remove(doc, target);
    MUT_OK
}
