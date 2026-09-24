//! Setting and removing attributes, by raw qualified name and by `(namespace,
//! local name)` - the DOM's two keys for the same node.
//!
//! Each of the four walks the element's attribute list exactly once: the scan
//! that looks for a match also finds the list's end, which is what the tail
//! argument to `Document::link_attr` is for.

#![forbid(unsafe_code)]

use super::ns::{resolve_ns, Ns, Resolved, NO_NS};
use super::{arena, assign_qname};
use crate::xml::chars::validate_chars;
use crate::xml::qname::{ns_decl_check, split_checked, xmlns_prefix, Split};
use crate::xml::{Document, MutStatus, NodeId, NodeType, Span, FLAG_NS_PENDING};

/// Build a fresh ATTRIBUTE (qname + value + namespace) and link it onto `el`
/// after `tail`, the last entry the caller's own scan reached.
fn build_attr(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    sp: &Split,
    val: &[u8],
    ns: Resolved,
    tail: Option<NodeId>,
) -> Result<NodeId, MutStatus> {
    let attr = arena(doc.new_node(NodeType::Attribute))?;
    let st = assign_qname(doc, attr, name, sp);
    if st != MutStatus::Ok {
        return Err(st);
    }
    arena(doc.set_value_bytes(attr, val))?;
    ns.write_attr(doc, attr);
    doc.link_attr(el, tail, attr);
    Ok(attr)
}

/// Whether two of the `(namespace, attribute)` keys are equal by namespace URI
/// and local name - the uniqueness rule (§3) a resolution checks before it
/// writes. Sorts `keys` in place.
pub(super) fn keys_repeat(doc: &Document, keys: &mut [(Span, NodeId)]) -> bool {
    let key = |&(ns, a): &(Span, NodeId)| (doc.span(ns), doc.local(a));
    keys.sort_unstable_by(|x, y| key(x).cmp(&key(y)));
    keys.windows(2).any(|w| key(&w[0]) == key(&w[1]))
}

/// Whether an attribute named `name` may hold `val`: anything but a namespace
/// declaration the §3 rules forbid ([`ns_decl_check`]), refused as
/// [`MutStatus::BadNsDecl`] with the clause it broke.
pub(super) fn decl_check(name: &[u8], val: &[u8]) -> Result<(), MutStatus> {
    match xmlns_prefix(name) {
        Some(p) => ns_decl_check(p, val).map_err(MutStatus::BadNsDecl),
        None => Ok(()),
    }
}

/// Whether an attribute of `el` other than `except` already has the key
/// (`ns`, `local`) - the uniqueness the parser enforces (§3). Asked only for a
/// DECIDED key: a prefix not yet resolvable (a detached element) has no
/// namespace to compare, and is checked when it is.
pub(super) fn key_taken(
    doc: &Document,
    el: NodeId,
    ns: &[u8],
    local: &[u8],
    except: Option<NodeId>,
) -> bool {
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if Some(attr) != except && attr_matches_ns(doc, attr, ns, local) {
            return true;
        }
        a = doc.next(attr);
    }
    false
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
    decl_check(name, val)?;
    if !val.is_empty() && !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let connected = doc.is_connected(el);
    let r = resolve_ns(doc, Some(el), name, &sp, true, connected)?;
    /* an existing attribute with the same raw QName -> replace its value */
    let mut tail = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if doc.qname(attr) == name {
            arena(doc.set_value_bytes(attr, val))?;
            r.write_attr(doc, attr);
            return Ok(attr);
        }
        tail = Some(attr);
        a = doc.next(attr);
    }
    /* No attribute has this QName, but one may have its key under another
     * prefix for the same URI (p:a beside q:a, both bound to one URI). A
     * pending one has no key yet: the insertion that decides it checks. */
    let local = &name[sp.local_off as usize..];
    if !r.pending && r.ns.len != 0 && key_taken(doc, el, doc.span(r.ns), local, None) {
        return Err(MutStatus::DuplicateAttr);
    }
    build_attr(doc, el, name, &sp, val, r, tail)
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
    /* A pending attribute's namespace is undecided, not empty: it has no key
     * to match (`set_attribute_ns("", "a")` used to overwrite a pending p:a). */
    doc.node(a).flags & FLAG_NS_PENDING == 0
        && doc.node(a).ns_uri.len as usize == ns.len()
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
    if !crate::xml::qname::ns_fits_name(ns, name, &sp) {
        return Err(MutStatus::BadNsName);
    }
    decl_check(name, val)?;
    if !val.is_empty() && !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let local = &name[sp.local_off as usize..];
    let mut tail = None;
    let mut a = doc.attrs(el);
    while let Some(attr) = a {
        if attr_matches_ns(doc, attr, ns, local) {
            arena(doc.set_value_bytes(attr, val))?;
            return Ok(attr);
        }
        tail = Some(attr);
        a = doc.next(attr);
    }
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        arena(doc.store(ns))?
    };
    build_attr(doc, el, name, &sp, val, Resolved::decided(nsv), tail)
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
