//! Building a detached node: the `Document#create_*` family.
//!
//! Everything here comes back unattached and, for an element, with NO namespace
//! decided - it takes one from the context it is first inserted into
//! (`super::ns`). That is what makes building a subtree bottom-up and attaching
//! it give the same tree as building it top-down.

#![forbid(unsafe_code)]

use super::edit::value_seq_ok;
use super::{arena, assign_qname};
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, Split};
use crate::xml::{Document, MutStatus, NodeId, NodeType, FLAG_DOM_LOOSE_NAME};

pub fn new_element(doc: &mut Document, name: &[u8]) -> Result<NodeId, MutStatus> {
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutStatus::BadName),
    };
    if sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MutStatus::BadName); /* xmlns: is not an element prefix */
    }
    let el = arena(doc.new_node(NodeType::Element))?;
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
    let el = arena(doc.new_node(NodeType::Element))?;
    arena(doc.assign_qname(el, name, prefix_len, local_off, local_len))?;
    if !ns.is_empty() {
        arena(doc.set_ns_bytes(el, ns))?;
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
    let n = arena(doc.new_node(ty))?;
    arena(doc.set_value_bytes(n, text))?;
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
    let pi = arena(doc.new_node(NodeType::Pi))?;
    let t = arena(doc.store(target))?;
    let d = arena(doc.store(data))?;
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
    /* The parser's rules, not looser ones: a DOCTYPE the factory accepted but
     * the parser rejects made `to_xml` output that did not re-parse. The name
     * is a QName (not any Name: "a:b:c" and ":a" were accepted); a PUBLIC id is
     * PubidChar only; a SYSTEM id may hold either quote, since the writer picks
     * the other one, but not both, which no literal can hold. */
    if crate::xml::qname::split_checked(name).is_none() {
        return Err(MutStatus::BadName);
    }
    if pub_id.is_some_and(|id| !crate::xml::chars::is_pubid(id)) {
        return Err(MutStatus::BadChars);
    }
    if sys_id.is_some_and(|id| !validate_chars(id) || (id.contains(&b'"') && id.contains(&b'\''))) {
        return Err(MutStatus::BadChars);
    }
    let dt = arena(doc.new_node(NodeType::Doctype))?;
    let nm = arena(doc.store(name))?;
    {
        let n = doc.node_mut(dt);
        n.local = nm;
        n.qname = nm;
    }
    if let Some(p) = pub_id {
        let pp = arena(doc.store(p))?;
        doc.node_mut(dt).prefix = pp;
    }
    if let Some(s) = sys_id {
        let sp = arena(doc.store(s))?;
        doc.node_mut(dt).value = sp;
    }
    Ok(dt)
}

/// A detached, empty DOCUMENT_FRAGMENT.
///
/// It exists because the HTML-to-XML cross-import needs one and everything else
/// it builds already comes from this module; without it that path reached into
/// the arena's `new_node` directly, which is the layer the arena's `pub(super)`
/// now closes off.
pub fn new_fragment(doc: &mut Document) -> Result<NodeId, MutStatus> {
    arena(doc.new_node(NodeType::Fragment))
}
