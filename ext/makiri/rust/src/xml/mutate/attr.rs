//! Setting and removing attributes, by raw qualified name and by `(namespace,
//! local name)` - the DOM's two keys for the same node.
//!
//! Each of the four finds its attribute with one scan ([`find_attr`]), which
//! also finds the list's end - what the tail argument to
//! `Document::link_attr` is for. `set_attribute` scans once more when it
//! ADDS an attribute, for the key-uniqueness check ([`key_taken`]).

#![forbid(unsafe_code)]

use super::ns::{resolve_ns, Ns, Resolved, NO_NS};
use super::{arena, assign_qname};
use crate::xml::chars::validate_chars;
use crate::xml::qname::{ns_decl_check, split_checked, xmlns_prefix, Split};
use crate::xml::{Document, MutStatus, NodeId, NodeType, Span, FLAG_NS_EXPLICIT, FLAG_NS_PENDING};

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
    assign_qname(doc, attr, name, sp)?;
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
    let key = AttrKey::Ns { ns, local };
    doc.attributes(el)
        .any(|attr| Some(attr) != except && key.matches(doc, attr))
}

/// How an attribute is looked up: by its raw qualified name, or by the DOM's
/// `(namespace, local name)` key - the module's two keys for the same node.
#[derive(Clone, Copy)]
enum AttrKey<'a> {
    QName(&'a [u8]),
    Ns { ns: &'a [u8], local: &'a [u8] },
}

impl AttrKey<'_> {
    fn matches(self, doc: &Document, a: NodeId) -> bool {
        match self {
            AttrKey::QName(q) => doc.qname(a) == q,
            AttrKey::Ns { ns, local } => attr_matches_ns(doc, a, ns, local),
        }
    }
}

/// Where the first attribute of `el` that `key` matches sits in its list: the
/// attribute and the one before it, or - when none matches - the last one,
/// which a new attribute is linked after.
enum AttrSlot {
    Found { prev: Option<NodeId>, attr: NodeId },
    Absent { tail: Option<NodeId> },
}

/// Find `el`'s first attribute `key` matches. Read-only: the edit that follows
/// is the caller's, once the walk is over.
fn find_attr(doc: &Document, el: NodeId, key: AttrKey<'_>) -> AttrSlot {
    let mut prev = None;
    for attr in doc.attributes(el) {
        if key.matches(doc, attr) {
            return AttrSlot::Found { prev, attr };
        }
        prev = Some(attr);
    }
    AttrSlot::Absent { tail: prev }
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
    let sp = split_checked(name).ok_or(MutStatus::BadName)?;
    decl_check(name, val)?;
    if !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    /* An attribute with this qualified name gets the value and nothing else,
     * as the DOM's setAttribute does: its namespace is its own, decided when
     * it was named. Re-deriving it here gave a second attribute the key of
     * another (`q:x` moved under a scope where `q` meant another attribute's
     * namespace), silently, and dropped a namespace set_attribute_ns gave. */
    let tail = match find_attr(doc, el, AttrKey::QName(name)) {
        AttrSlot::Found { attr, .. } => {
            arena(doc.set_value_bytes(attr, val))?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    let connected = doc.is_connected(el);
    let r = resolve_ns(doc, Some(el), name, &sp, true, connected)?;
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
    remove_attr_by(doc, el, AttrKey::QName(name))
}

/// Unlink `el`'s attribute that `key` matches; `true` when there was one. The
/// shared body of the two removers, which differ only in the key.
fn remove_attr_by(doc: &mut Document, el: NodeId, key: AttrKey<'_>) -> bool {
    if doc.type_(el) != Some(NodeType::Element) {
        return false;
    }
    match find_attr(doc, el, key) {
        AttrSlot::Found { prev, attr } => {
            doc.unlink_attr(el, prev, attr);
            true
        }
        AttrSlot::Absent { .. } => false,
    }
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
    let sp = split_checked(name).ok_or(MutStatus::BadName)?;
    if !crate::xml::qname::ns_fits_name(ns, name, &sp) {
        return Err(MutStatus::BadNsName);
    }
    decl_check(name, val)?;
    if !validate_chars(val) {
        return Err(MutStatus::BadChars);
    }
    let local = &name[sp.local_off as usize..];
    let tail = match find_attr(doc, el, AttrKey::Ns { ns, local }) {
        AttrSlot::Found { attr, .. } => {
            arena(doc.set_value_bytes(attr, val))?;
            return Ok(attr);
        }
        AttrSlot::Absent { tail } => tail,
    };
    /* no match: copy the namespace into the arena only now */
    let nsv: Ns = if ns.is_empty() {
        NO_NS
    } else {
        arena(doc.store(ns))?
    };
    let attr = build_attr(doc, el, name, &sp, val, Resolved::decided(nsv), tail)?;
    doc.node_mut(attr).flags |= FLAG_NS_EXPLICIT;
    Ok(attr)
}

/// Remove `el`'s attribute keyed by `(ns, local)`; `true` when one was removed.
pub fn remove_attribute_ns(doc: &mut Document, el: NodeId, ns: &[u8], local: &[u8]) -> bool {
    remove_attr_by(doc, el, AttrKey::Ns { ns, local })
}
