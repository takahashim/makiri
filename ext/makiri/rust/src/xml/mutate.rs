//! Mutation primitives (mkr_xml_mutate.c). Every primitive validates and
//! allocates BEFORE changing any link, so a failure leaves the tree untouched.
//! Inherently unsafe: it walks and relinks the C-layout nodes.

/* One precondition throughout: every node passed in is live and allocated from
 * `doc`'s arena. The primitives validate everything else themselves - that is
 * what the module header means by allocating before relinking. */
#![allow(clippy::missing_safety_doc)]

use crate::xml::arena::{arena_bytes, arena_node, preorder_next, qname_assign};
use crate::falloc::Reserve;
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, value_seq_ok, xmlns_prefix};
use crate::xml::{
    bytes, empty, node_local, node_ns, node_qname, node_value, qname_from, qname_of, Doc, Node,
    QName, FLAG_DOM_LOOSE_NAME, FLAG_NS_RESOLVED, MUT_BAD_CHARS, MUT_BAD_NAME,
    MUT_BAD_NS_DECL, MUT_CYCLE,
    MUT_HIERARCHY, MUT_OK, MUT_OOM, MUT_TYPE, MUT_UNBOUND_NS, T_ATTRIBUTE, T_CDATA, T_COMMENT,
    T_DOCTYPE, T_DOCUMENT, T_ELEMENT, T_PI, T_TEXT, XMLNS_NS_URI, XML_NS_URI,
};
use core::ffi::c_char;
use core::ptr;

type Ns = (*const c_char, u32);

const NO_NS: Ns = (ptr::null(), 0);

#[inline]
fn xml_ns() -> Ns {
    (XML_NS_URI.as_ptr() as *const c_char, XML_NS_URI.len() as u32)
}
#[inline]
fn xmlns_ns() -> Ns {
    (XMLNS_NS_URI.as_ptr() as *const c_char, XMLNS_NS_URI.len() as u32)
}

/// Nearest in-scope binding for `prefix` ("" = default) at or above `node`.
unsafe fn resolve_in_scope(node: *const Node, prefix: &[u8]) -> Option<Ns> {
    let mut e = node;
    while !e.is_null() {
        if (*e).type_ == T_ELEMENT {
            let mut a = (*e).attrs;
            while !a.is_null() {
                if let Some(p) = xmlns_prefix(node_qname(a)) {
                    if p == prefix {
                        let u = if (*a).value.is_null() { empty() } else { (*a).value };
                        return Some((u, (*a).value_len));
                    }
                }
                a = (*a).next;
            }
        }
        e = (*e).parent;
    }
    None
}

/// `node`'s topmost ancestor is the document node.
unsafe fn is_connected(node: *const Node) -> bool {
    let mut top = node;
    while !(*top).parent.is_null() {
        top = (*top).parent;
    }
    (*top).type_ == T_DOCUMENT
}

/// Resolve `qn` applied at `scope` (mirrors the parser's §7 rules). An unbound
/// prefix is an error only when connected; deferred (unresolved) otherwise.
unsafe fn resolve_ns(scope: *const Node, qn: &QName, is_attr: bool, connected: bool) -> Result<Ns, i32> {
    let qname = bytes(qn.qname, qn.qname_len);
    let prefix = bytes(qn.prefix, qn.prefix_len);
    if is_attr && xmlns_prefix(qname).is_some() {
        return Ok(xmlns_ns());
    }
    if qn.prefix_len == 0 {
        if is_attr {
            return Ok(NO_NS); /* unprefixed attribute -> no namespace */
        }
        return Ok(match resolve_in_scope(scope, b"") {
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
    match resolve_in_scope(scope, prefix) {
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
unsafe fn assign_qname(doc: *mut Doc, node: *mut Node, qn: &QName) -> i32 {
    if qname_assign(doc, node, qn) == 0 {
        MUT_OK
    } else {
        MUT_OOM
    }
}

#[inline]
unsafe fn set_ns(n: *mut Node, ns: Ns) {
    (*n).ns_uri = ns.0;
    (*n).ns_uri_len = ns.1;
}

/// Unlink attribute `a` (predecessor `prev`, null if head) from `el`.
unsafe fn unlink_attr(el: *mut Node, prev: *mut Node, a: *mut Node) {
    if !prev.is_null() {
        (*prev).next = (*a).next;
    } else {
        (*el).attrs = (*a).next;
    }
    (*a).next = ptr::null_mut();
    (*a).parent = ptr::null_mut();
}

pub unsafe fn detach(node: *mut Node) {
    let parent = (*node).parent;
    if parent.is_null() {
        return;
    }
    if (*node).type_ == T_ATTRIBUTE {
        let mut prev: *mut Node = ptr::null_mut();
        let mut a = (*parent).attrs;
        while !a.is_null() {
            if a == node {
                unlink_attr(parent, prev, a);
                break;
            }
            prev = a;
            a = (*a).next;
        }
        (*node).next = ptr::null_mut();
        (*node).parent = ptr::null_mut();
        return;
    }
    if !(*node).prev.is_null() {
        (*(*node).prev).next = (*node).next;
    } else {
        (*parent).first_child = (*node).next;
    }
    if !(*node).next.is_null() {
        (*(*node).next).prev = (*node).prev;
    } else {
        (*parent).last_child = (*node).prev;
    }
    (*node).parent = ptr::null_mut();
    (*node).prev = ptr::null_mut();
    (*node).next = ptr::null_mut();
}

pub unsafe fn rename(doc: *mut Doc, node: *mut Node, name: &[u8]) -> i32 {
    if (*node).type_ != T_ELEMENT && (*node).type_ != T_ATTRIBUTE {
        return MUT_TYPE;
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MUT_BAD_NAME,
    };
    let qn = qname_from(name, &sp);
    let is_attr = (*node).type_ == T_ATTRIBUTE;
    let scope: *const Node = if is_attr { (*node).parent } else { node };
    let connected = !scope.is_null() && is_connected(scope);
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
    (*node).flags &= !FLAG_DOM_LOOSE_NAME;
    /* A rename picks a new prefix, so it decides a new URI from the scope the
     * node is in right now - and that decision is the node's identity from here
     * (an element's; an attribute follows its element). */
    if connected && !is_attr {
        (*node).flags |= FLAG_NS_RESOLVED;
    }
    MUT_OK
}

unsafe fn append_attr(el: *mut Node, attr: *mut Node) {
    (*attr).parent = el;
    if (*el).attrs.is_null() {
        (*el).attrs = attr;
        return;
    }
    let mut t = (*el).attrs;
    while !(*t).next.is_null() {
        t = (*t).next;
    }
    (*t).next = attr;
}

/// Build a fresh ATTRIBUTE (qname + value + namespace) and append it to `el`.
unsafe fn build_attr(
    doc: *mut Doc,
    el: *mut Node,
    qn: &QName,
    val: &[u8],
    ns: Ns,
    out: *mut *mut Node,
) -> i32 {
    let attr = arena_node(doc, T_ATTRIBUTE);
    if attr.is_null() {
        return MUT_OOM;
    }
    let st = assign_qname(doc, attr, qn);
    if st != MUT_OK {
        return st;
    }
    let nv = arena_bytes(doc, val);
    if nv.is_null() {
        return MUT_OOM;
    }
    (*attr).value = nv;
    (*attr).value_len = val.len() as u32;
    set_ns(attr, ns);
    append_attr(el, attr);
    if !out.is_null() {
        *out = attr;
    }
    MUT_OK
}

pub unsafe fn set_attribute(
    doc: *mut Doc,
    el: *mut Node,
    name: &[u8],
    val: &[u8],
    out: *mut *mut Node,
) -> i32 {
    if !out.is_null() {
        *out = ptr::null_mut();
    }
    if (*el).type_ != T_ELEMENT {
        return MUT_TYPE;
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MUT_BAD_NAME,
    };
    let qn = qname_from(name, &sp);
    /* xmlns:foo="" must not bind a prefix to the empty namespace */
    if val.is_empty() && sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return MUT_BAD_NS_DECL;
    }
    if !val.is_empty() && !validate_chars(val) {
        return MUT_BAD_CHARS;
    }
    let ns = match resolve_ns(el, &qn, true, is_connected(el)) {
        Ok(ns) => ns,
        Err(st) => return st,
    };
    /* an existing attribute with the same raw QName -> replace its value */
    let mut a = (*el).attrs;
    while !a.is_null() {
        if node_qname(a) == name {
            let nv = arena_bytes(doc, val);
            if nv.is_null() {
                return MUT_OOM;
            }
            (*a).value = nv;
            (*a).value_len = val.len() as u32;
            set_ns(a, ns);
            if !out.is_null() {
                *out = a;
            }
            return MUT_OK;
        }
        a = (*a).next;
    }
    build_attr(doc, el, &qn, val, ns, out)
}

pub unsafe fn remove_attribute(el: *mut Node, name: &[u8]) -> i32 {
    if (*el).type_ != T_ELEMENT {
        return 0;
    }
    let mut prev: *mut Node = ptr::null_mut();
    let mut a = (*el).attrs;
    while !a.is_null() {
        if node_qname(a) == name {
            unlink_attr(el, prev, a);
            return 1;
        }
        prev = a;
        a = (*a).next;
    }
    0
}

/// `a` is keyed by (ns, local) - the DOM key; an empty wanted namespace
/// matches an attribute with no namespace.
unsafe fn attr_matches_ns(a: *const Node, ns: &[u8], local: &[u8]) -> bool {
    (*a).ns_uri_len as usize == ns.len() && (ns.is_empty() || node_ns(a) == ns) && node_local(a) == local
}

pub unsafe fn set_attribute_ns(
    doc: *mut Doc,
    el: *mut Node,
    ns: &[u8],
    name: &[u8],
    val: &[u8],
    out: *mut *mut Node,
) -> i32 {
    if !out.is_null() {
        *out = ptr::null_mut();
    }
    if (*el).type_ != T_ELEMENT {
        return MUT_TYPE;
    }
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MUT_BAD_NAME,
    };
    let qn = qname_from(name, &sp);
    if !val.is_empty() && !validate_chars(val) {
        return MUT_BAD_CHARS;
    }
    let local = &name[sp.local_off as usize..];
    let mut a = (*el).attrs;
    while !a.is_null() {
        if attr_matches_ns(a, ns, local) {
            let nv = arena_bytes(doc, val);
            if nv.is_null() {
                return MUT_OOM;
            }
            (*a).value = nv;
            (*a).value_len = val.len() as u32;
            if !out.is_null() {
                *out = a;
            }
            return MUT_OK;
        }
        a = (*a).next;
    }
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        let u = arena_bytes(doc, ns);
        if u.is_null() {
            return MUT_OOM;
        }
        (u, ns.len() as u32)
    };
    build_attr(doc, el, &qn, val, nsv, out)
}

pub unsafe fn remove_attribute_ns(el: *mut Node, ns: &[u8], local: &[u8]) -> i32 {
    if (*el).type_ != T_ELEMENT {
        return 0;
    }
    let mut prev: *mut Node = ptr::null_mut();
    let mut a = (*el).attrs;
    while !a.is_null() {
        if attr_matches_ns(a, ns, local) {
            unlink_attr(el, prev, a);
            return 1;
        }
        prev = a;
        a = (*a).next;
    }
    0
}

pub unsafe fn set_content(doc: *mut Doc, node: *mut Node, text: &[u8]) -> i32 {
    if !text.is_empty() && !validate_chars(text) {
        return MUT_BAD_CHARS;
    }
    match (*node).type_ {
        T_TEXT | T_CDATA | T_COMMENT | T_PI => {
            if !value_seq_ok((*node).type_, text) {
                return MUT_BAD_CHARS;
            }
            let nv = arena_bytes(doc, text);
            if nv.is_null() {
                return MUT_OOM;
            }
            (*node).value = nv;
            (*node).value_len = text.len() as u32;
            MUT_OK
        }
        T_ELEMENT => {
            /* build the replacement TEXT node FIRST, so an OOM leaves the
             * children intact */
            let mut t: *mut Node = ptr::null_mut();
            if !text.is_empty() {
                let nv = arena_bytes(doc, text);
                if nv.is_null() {
                    return MUT_OOM;
                }
                t = arena_node(doc, T_TEXT);
                if t.is_null() {
                    return MUT_OOM;
                }
                (*t).value = nv;
                (*t).value_len = text.len() as u32;
            }
            let mut c = (*node).first_child;
            while !c.is_null() {
                let nx = (*c).next;
                (*c).parent = ptr::null_mut();
                (*c).prev = ptr::null_mut();
                (*c).next = ptr::null_mut();
                c = nx;
            }
            (*node).first_child = t;
            (*node).last_child = t;
            if !t.is_null() {
                (*t).parent = node;
            }
            MUT_OK
        }
        _ => MUT_TYPE,
    }
}

/* ============================ Phase 2: building ============================ */

pub unsafe fn new_element(doc: *mut Doc, name: &[u8], out: *mut *mut Node) -> i32 {
    *out = ptr::null_mut();
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return MUT_BAD_NAME,
    };
    if sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return MUT_BAD_NAME; /* xmlns: is not an element prefix */
    }
    let el = arena_node(doc, T_ELEMENT);
    if el.is_null() {
        return MUT_OOM;
    }
    let st = assign_qname(doc, el, &qname_from(name, &sp));
    if st != MUT_OK {
        return st;
    }
    *out = el; /* ns_uri stays unresolved until insertion */
    MUT_OK
}

pub unsafe fn new_loose_dom_element(doc: *mut Doc, qn: *const QName, ns: &[u8], out: *mut *mut Node) -> i32 {
    *out = ptr::null_mut();
    if qn.is_null() || (*qn).qname_len == 0 || (*qn).local_len == 0 {
        return MUT_BAD_NAME;
    }
    let qn = &*qn;
    let (q0, l0) = (qn.qname as usize, qn.local as usize);
    if !(l0 >= q0 && qn.local_len <= qn.qname_len && (l0 - q0) <= (qn.qname_len - qn.local_len) as usize) {
        return MUT_BAD_NAME;
    }
    let el = arena_node(doc, T_ELEMENT);
    if el.is_null() {
        return MUT_OOM;
    }
    let st = assign_qname(doc, el, qn);
    if st != MUT_OK {
        return st;
    }
    if !ns.is_empty() {
        let u = arena_bytes(doc, ns);
        if u.is_null() {
            return MUT_OOM;
        }
        set_ns(el, (u, ns.len() as u32));
    }
    (*el).flags |= FLAG_DOM_LOOSE_NAME;
    *out = el;
    MUT_OK
}

pub unsafe fn new_chardata(doc: *mut Doc, ty: u32, text: &[u8], out: *mut *mut Node) -> i32 {
    *out = ptr::null_mut();
    if ty != T_TEXT && ty != T_CDATA && ty != T_COMMENT {
        return MUT_TYPE;
    }
    if !text.is_empty() && !validate_chars(text) {
        return MUT_BAD_CHARS;
    }
    if !value_seq_ok(ty, text) {
        return MUT_BAD_CHARS;
    }
    let n = arena_node(doc, ty);
    if n.is_null() {
        return MUT_OOM;
    }
    let v = arena_bytes(doc, text);
    if v.is_null() {
        return MUT_OOM;
    }
    (*n).value = v;
    (*n).value_len = text.len() as u32;
    *out = n;
    MUT_OK
}

pub unsafe fn new_pi(doc: *mut Doc, target: &[u8], data: &[u8], out: *mut *mut Node) -> i32 {
    *out = ptr::null_mut();
    if !crate::xml::chars::validate_name(target) || crate::xml::chars::is_reserved_pi_target(target) {
        return MUT_BAD_NAME;
    }
    if !data.is_empty() && !validate_chars(data) {
        return MUT_BAD_CHARS;
    }
    if !value_seq_ok(T_PI, data) {
        return MUT_BAD_CHARS;
    }
    let pi = arena_node(doc, T_PI);
    if pi.is_null() {
        return MUT_OOM;
    }
    let t = arena_bytes(doc, target);
    if t.is_null() {
        return MUT_OOM;
    }
    let d = arena_bytes(doc, data);
    if d.is_null() {
        return MUT_OOM;
    }
    (*pi).local = t;
    (*pi).local_len = target.len() as u32;
    (*pi).value = d;
    (*pi).value_len = data.len() as u32;
    *out = pi;
    MUT_OK
}

pub unsafe fn new_document_type(
    doc: *mut Doc,
    name: &[u8],
    pub_id: Option<&[u8]>,
    sys_id: Option<&[u8]>,
    out: *mut *mut Node,
) -> i32 {
    *out = ptr::null_mut();
    if !crate::xml::chars::validate_name(name) {
        return MUT_BAD_NAME;
    }
    for id in [pub_id, sys_id].into_iter().flatten() {
        if !id.is_empty() && (!validate_chars(id) || id.contains(&b'"')) {
            return MUT_BAD_CHARS;
        }
    }
    let dt = arena_node(doc, T_DOCTYPE);
    if dt.is_null() {
        return MUT_OOM;
    }
    let nm = arena_bytes(doc, name);
    if nm.is_null() {
        return MUT_OOM;
    }
    (*dt).local = nm;
    (*dt).qname = nm;
    (*dt).local_len = name.len() as u32;
    (*dt).qname_len = name.len() as u32;
    if let Some(p) = pub_id {
        let pp = arena_bytes(doc, p);
        if pp.is_null() {
            return MUT_OOM;
        }
        (*dt).prefix = pp;
        (*dt).prefix_len = p.len() as u32;
    }
    if let Some(s) = sys_id {
        let sp = arena_bytes(doc, s);
        if sp.is_null() {
            return MUT_OOM;
        }
        (*dt).value = sp;
        (*dt).value_len = s.len() as u32;
    }
    *out = dt;
    MUT_OK
}

/// Resolve the namespace of element `e` and its attributes.
///
/// `commit` selects the pass: false only computes (to find out whether every
/// prefix in the subtree binds), true writes the resolved URIs. See
/// `resolve_subtree`.
unsafe fn resolve_node_ns(e: *mut Node, connected: bool, commit: bool) -> i32 {
    if (*e).flags & FLAG_DOM_LOOSE_NAME == 0 {
        let eq = qname_of(e);
        match resolve_ns(e, &eq, false, connected) {
            Ok(ns) => {
                if commit {
                    set_ns(e, ns)
                }
            }
            Err(st) => return st,
        }
    }
    let mut a = (*e).attrs;
    while !a.is_null() {
        let aq = qname_of(a);
        match resolve_ns(e, &aq, true, connected) {
            Ok(ns) => {
                if commit {
                    set_ns(a, ns)
                }
            }
            Err(st) => return st,
        }
        a = (*a).next;
    }
    /* Only mark once connected: resolution inside a still-detached fragment is
     * deferred (an unbound prefix is not an error there), so the node must stay
     * open to being resolved again when the fragment joins the document. */
    if commit && connected {
        (*e).flags |= FLAG_NS_RESOLVED;
    }
    MUT_OK
}

/// True once `e`'s namespace has been decided - by the parser, or by resolving
/// it against the context it was first inserted into. From then on the URI is
/// the node's identity, so a later move must NOT re-derive it: that is what
/// makes namespaceURI survive a move the way the DOM and browsers have it, and
/// the serializer emits whatever declarations the output needs.
unsafe fn ns_is_decided(e: *const Node) -> bool {
    (*e).flags & FLAG_NS_RESOLVED != 0
}

/// Re-resolve every element in `root`'s subtree, all-or-nothing.
///
/// The walk writes as it goes, so a bare single pass that fails partway leaves
/// the subtree half-rewritten: the elements before the unbound prefix carry URIs
/// resolved against a scope the tree is not in, while the rest keep the old
/// ones. That state is invisible to serialization (only prefixes are written)
/// but wrong for XPath, which matches on the resolved URI. So: one pass that
/// only computes, and - only if every prefix binds - a second that writes.
unsafe fn resolve_subtree(root: *mut Node, connected: bool) -> i32 {
    /* Both passes run the SAME body - that is the point of the loop rather than
     * two functions. If the check could drift from the commit, that drift would
     * be the bug. */
    for commit in [false, true] {
        let mut cur = root;
        while !cur.is_null() {
            if (*cur).type_ == T_ELEMENT && !ns_is_decided(cur) {
                let st = resolve_node_ns(cur, connected, commit);
                if st != MUT_OK {
                    return st; /* commit == false: nothing written yet */
                }
            }
            cur = preorder_next(root, cur);
        }
    }
    MUT_OK
}

/// Resolve `node`'s subtree as if it were a child of `context`, WITHOUT
/// linking it (borrow node.parent for the ancestor walk, then restore).
unsafe fn resolve_into(node: *mut Node, context: *mut Node) -> i32 {
    let saved = (*node).parent;
    (*node).parent = context;
    let st = resolve_subtree(node, is_connected(node));
    (*node).parent = saved;
    st
}

/// One arena copy of `src` (own fields + attributes, NOT children), INCLUDING
/// its resolved namespace URI. A copy keeps the namespace it had: the URI is the
/// node identity, so neither cloneNode nor importNode re-derives it from wherever
/// the copy lands - what the DOM and browsers do.
unsafe fn copy_one(doc: *mut Doc, src: *const Node) -> *mut Node {
    let n = arena_node(doc, (*src).type_);
    if n.is_null() {
        return n;
    }
    if !(*src).qname.is_null() && (*src).qname_len > 0 {
        let qn = qname_of(src);
        if assign_qname(doc, n, &qn) != MUT_OK {
            return ptr::null_mut();
        }
    } else if !(*src).local.is_null() && (*src).local_len > 0 {
        let t = arena_bytes(doc, node_local(src)); /* PI target */
        if t.is_null() {
            return ptr::null_mut();
        }
        (*n).local = t;
        (*n).local_len = (*src).local_len;
    }
    if (*src).value_len > 0 {
        let v = arena_bytes(doc, node_value(src));
        if v.is_null() {
            return ptr::null_mut();
        }
        (*n).value = v;
        (*n).value_len = (*src).value_len;
    } else if !(*src).value.is_null() {
        (*n).value = empty();
    }
    (*n).flags = (*src).flags;
    if !(*src).ns_uri.is_null() && (*src).ns_uri_len > 0 {
        let u = arena_bytes(doc, node_ns(src));
        if u.is_null() {
            return ptr::null_mut();
        }
        set_ns(n, (u, (*src).ns_uri_len));
    }
    /* copy attributes (each an arena node), preserving order */
    let mut tail: *mut Node = ptr::null_mut();
    let mut a = (*src).attrs;
    while !a.is_null() {
        let ca = copy_one(doc, a); /* an attribute has no children/attrs */
        if ca.is_null() {
            return ptr::null_mut();
        }
        (*ca).parent = n;
        if tail.is_null() {
            (*n).attrs = ca;
        } else {
            (*tail).next = ca;
        }
        tail = ca;
        a = (*a).next;
    }
    n
}

/// Deep copy of `src`'s subtree (iterative; no recursion).
unsafe fn deep_copy(doc: *mut Doc, src: *const Node) -> Result<*mut Node, i32> {
    let root = copy_one(doc, src);
    if root.is_null() {
        return Err(MUT_OOM);
    }
    let mut stack: Vec<(*const Node, *mut Node)> = Vec::new();
    if stack.mkr_reserve(1).is_err() {
        return Err(MUT_OOM);
    }
    stack.push((src, root));
    while let Some((s, d)) = stack.pop() {
        let mut dtail: *mut Node = ptr::null_mut();
        let mut sc = (*s).first_child;
        while !sc.is_null() {
            let dc = copy_one(doc, sc);
            if dc.is_null() {
                return Err(MUT_OOM); /* partial copy abandoned in the arena */
            }
            (*dc).parent = d;
            if !dtail.is_null() {
                (*dtail).next = dc;
                (*dc).prev = dtail;
            } else {
                (*d).first_child = dc;
            }
            (*d).last_child = dc;
            dtail = dc;
            if !(*sc).first_child.is_null() {
                if stack.mkr_reserve(1).is_err() {
                    return Err(MUT_OOM);
                }
                stack.push((sc, dc));
            }
            sc = (*sc).next;
        }
    }
    Ok(root)
}

#[inline]
unsafe fn copied(r: Result<*mut Node, i32>, out: *mut *mut Node) -> i32 {
    match r {
        Ok(n) => {
            *out = n;
            MUT_OK
        }
        Err(st) => {
            *out = ptr::null_mut();
            st
        }
    }
}

pub unsafe fn import_subtree(doc: *mut Doc, src: *const Node, out: *mut *mut Node) -> i32 {
    copied(deep_copy(doc, src), out)
}

pub unsafe fn clone_node(doc: *mut Doc, src: *const Node, deep: bool, out: *mut *mut Node) -> i32 {
    if deep {
        return copied(deep_copy(doc, src), out);
    }
    *out = copy_one(doc, src);
    if (*out).is_null() {
        MUT_OOM
    } else {
        MUT_OK
    }
}

pub unsafe fn copy_node(doc: *mut Doc, src: *const Node, deep: bool, out: *mut *mut Node) -> i32 {
    if deep {
        return copied(deep_copy(doc, src), out);
    }
    *out = copy_one(doc, src);
    if (*out).is_null() {
        MUT_OOM
    } else {
        MUT_OK
    }
}

/* ---- insertion ---- */

#[inline]
unsafe fn is_insertable(node: *const Node) -> bool {
    matches!((*node).type_, T_ELEMENT | T_TEXT | T_CDATA | T_COMMENT | T_PI | T_DOCTYPE)
}

/// WHATWG doctype ordering at the document node (fail-closed).
unsafe fn check_doc_child_order(
    container: *const Node,
    node: *const Node,
    before: *const Node,
    exclude: *const Node,
) -> i32 {
    if (*container).type_ != T_DOCUMENT {
        return if (*node).type_ == T_DOCTYPE { MUT_HIERARCHY } else { MUT_OK };
    }
    if (*node).type_ == T_DOCTYPE {
        let mut c = (*container).first_child as *const Node;
        while !c.is_null() {
            if c != exclude && c != node && (*c).type_ == T_DOCTYPE {
                return MUT_HIERARCHY; /* at most one */
            }
            c = (*c).next;
        }
        /* no element before the doctype */
        let mut c = (*container).first_child as *const Node;
        while c != before && !c.is_null() {
            if c != exclude && c != node && (*c).type_ == T_ELEMENT {
                return MUT_HIERARCHY;
            }
            c = (*c).next;
        }
        return MUT_OK;
    }
    if (*node).type_ == T_ELEMENT {
        let mut c = before;
        while !c.is_null() {
            if c != exclude && c != node && (*c).type_ == T_DOCTYPE {
                return MUT_HIERARCHY;
            }
            c = (*c).next;
        }
    }
    MUT_OK
}

unsafe fn would_cycle(container: *const Node, node: *const Node) -> bool {
    let mut p = container;
    while !p.is_null() {
        if p == node {
            return true;
        }
        p = (*p).parent;
    }
    false
}

unsafe fn doc_root_ok(container: *const Node, node: *const Node, exclude: *const Node) -> bool {
    if (*container).type_ != T_DOCUMENT || (*node).type_ != T_ELEMENT {
        return true;
    }
    let mut c = (*container).first_child as *const Node;
    while !c.is_null() {
        if c != exclude && c != node && (*c).type_ == T_ELEMENT {
            return false;
        }
        c = (*c).next;
    }
    true
}

/// Re-derive doc.root / doc.doctype from the tree after a change at the
/// document node.
unsafe fn sync_doc_meta(doc: *mut Doc, container: *const Node) {
    if doc.is_null() || !ptr::eq(container, (*doc).doc_node) {
        return;
    }
    (*doc).root = ptr::null_mut();
    (*doc).doctype = ptr::null_mut();
    let mut c = (*(*doc).doc_node).first_child;
    while !c.is_null() {
        if (*doc).root.is_null() && (*c).type_ == T_ELEMENT {
            (*doc).root = c;
        }
        if (*doc).doctype.is_null() && (*c).type_ == T_DOCTYPE {
            (*doc).doctype = c;
        }
        c = (*c).next;
    }
}

/// Validation + namespace resolution for inserting `node` under `container`
/// before `before` (null = append), replacing `exclude` (or null). No
/// structural change.
unsafe fn prepare_insert(container: *mut Node, node: *mut Node, before: *const Node, exclude: *const Node) -> i32 {
    if !is_insertable(node) {
        return MUT_HIERARCHY;
    }
    if (*container).type_ != T_ELEMENT && (*container).type_ != T_DOCUMENT {
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

/// The ONE place the doubly-linked child list is written.
unsafe fn splice_between(container: *mut Node, node: *mut Node, prev: *mut Node, next: *mut Node) {
    (*node).parent = container;
    (*node).prev = prev;
    (*node).next = next;
    if !prev.is_null() {
        (*prev).next = node;
    } else {
        (*container).first_child = node;
    }
    if !next.is_null() {
        (*next).prev = node;
    } else {
        (*container).last_child = node;
    }
}

pub unsafe fn insert_child(doc: *mut Doc, parent: *mut Node, node: *mut Node) -> i32 {
    let st = prepare_insert(parent, node, ptr::null(), ptr::null());
    if st != MUT_OK {
        return st;
    }
    detach(node);
    splice_between(parent, node, (*parent).last_child, ptr::null_mut());
    sync_doc_meta(doc, parent);
    MUT_OK
}

pub unsafe fn insert_before(doc: *mut Doc, r: *mut Node, node: *mut Node) -> i32 {
    if node == r {
        return MUT_OK;
    }
    let container = (*r).parent;
    if container.is_null() {
        return MUT_HIERARCHY;
    }
    let st = prepare_insert(container, node, r, ptr::null());
    if st != MUT_OK {
        return st;
    }
    detach(node);
    splice_between(container, node, (*r).prev, r);
    sync_doc_meta(doc, container);
    MUT_OK
}

pub unsafe fn insert_after(doc: *mut Doc, r: *mut Node, node: *mut Node) -> i32 {
    if node == r {
        return MUT_OK;
    }
    let container = (*r).parent;
    if container.is_null() {
        return MUT_HIERARCHY;
    }
    let st = prepare_insert(container, node, (*r).next, ptr::null());
    if st != MUT_OK {
        return st;
    }
    detach(node);
    splice_between(container, node, r, (*r).next);
    sync_doc_meta(doc, container);
    MUT_OK
}

pub unsafe fn replace_node(doc: *mut Doc, r: *mut Node, node: *mut Node) -> i32 {
    let container = (*r).parent;
    if container.is_null() {
        return MUT_HIERARCHY;
    }
    if node == r {
        return MUT_OK;
    }
    let st = prepare_insert(container, node, r, r);
    if st != MUT_OK {
        return st;
    }
    detach(node);
    splice_between(container, node, (*r).prev, (*r).next);
    (*r).parent = ptr::null_mut();
    (*r).prev = ptr::null_mut();
    (*r).next = ptr::null_mut();
    sync_doc_meta(doc, container);
    MUT_OK
}

pub unsafe fn remove(doc: *mut Doc, node: *mut Node) {
    let parent = (*node).parent;
    detach(node);
    if !doc.is_null() {
        sync_doc_meta(doc, parent);
    }
}

unsafe fn element_child_count(parent: *const Node, exclude: *const Node) -> usize {
    let mut n = 0;
    let mut c = (*parent).first_child as *const Node;
    while !c.is_null() {
        if c != exclude && (*c).type_ == T_ELEMENT {
            n += 1;
        }
        c = (*c).next;
    }
    n
}

/// Replace `target` with the CHILDREN of `frag`, atomically (fail-closed).
pub unsafe fn replace_with_fragment(doc: *mut Doc, target: *mut Node, frag: *mut Node) -> i32 {
    let container = (*target).parent;
    if container.is_null() {
        return MUT_HIERARCHY;
    }
    /* --- validation pass: no links change until it all passes */
    if (*container).type_ == T_DOCUMENT {
        if element_child_count(frag, ptr::null()) + element_child_count(container, target) > 1 {
            return MUT_HIERARCHY;
        }
        let mut c = (*frag).first_child as *const Node;
        while !c.is_null() {
            if (*c).type_ == T_DOCTYPE {
                return MUT_HIERARCHY;
            }
            c = (*c).next;
        }
    }
    let mut c = (*frag).first_child;
    while !c.is_null() {
        let st = prepare_insert(container, c, target, target);
        if st != MUT_OK {
            return st;
        }
        c = (*c).next;
    }
    /* --- commit pass: every child takes target's slot in fragment order */
    loop {
        let c = (*frag).first_child;
        if c.is_null() {
            break;
        }
        detach(c);
        splice_between(container, c, (*target).prev, target);
    }
    remove(doc, target);
    MUT_OK
}
