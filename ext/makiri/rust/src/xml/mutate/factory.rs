//! Building a detached node: the `Document#create_*` family.
//!
//! Everything here comes back unattached and, for an element, with NO namespace
//! decided - it takes one from the context it is first inserted into
//! (`super::ns`). That is what makes building a subtree bottom-up and attaching
//! it give the same tree as building it top-down.

#![forbid(unsafe_code)]

use super::assign_qname;
use super::edit::value_seq_ok;
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, Split};
use crate::xml::{ArenaKind, Document, MutError, NodeFlags, NodeId};

pub fn new_element(doc: &mut Document, name: &[u8]) -> Result<NodeId, MutError> {
    let sp = match split_checked(name) {
        Some(s) => s,
        None => return Err(MutError::BadName),
    };
    if sp.prefix_len == 5 && &name[..5] == b"xmlns" {
        return Err(MutError::BadName); /* xmlns: is not an element prefix */
    }
    let el = doc.new_node(ArenaKind::Element)?;
    assign_qname(doc, el, name, &sp)?;
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
) -> Result<NodeId, MutError> {
    let Split {
        prefix_len,
        local_off,
        local_len,
    } = sp;
    if name.is_empty() || local_len == 0 {
        return Err(MutError::BadName);
    }
    if local_off as usize + local_len as usize > name.len() || prefix_len as usize > name.len() {
        return Err(MutError::BadName);
    }
    let el = doc.new_node(ArenaKind::Element)?;
    doc.assign_qname(el, name, prefix_len, local_off, local_len)?;
    if !ns.is_empty() {
        doc.set_ns_bytes(el, ns)?;
    }
    doc.node_mut(el).flags.insert(NodeFlags::DOM_LOOSE_NAME);
    Ok(el)
}

pub fn new_chardata(doc: &mut Document, ty: ArenaKind, text: &[u8]) -> Result<NodeId, MutError> {
    if ty != ArenaKind::Text && ty != ArenaKind::CDataSection && ty != ArenaKind::Comment {
        return Err(MutError::Type);
    }
    if !validate_chars(text) {
        return Err(MutError::BadChars);
    }
    if !value_seq_ok(ty, text) {
        return Err(MutError::BadChars);
    }
    let n = doc.new_node(ty)?;
    doc.set_value_bytes(n, text)?;
    Ok(n)
}

pub fn new_pi(doc: &mut Document, target: &[u8], data: &[u8]) -> Result<NodeId, MutError> {
    if !crate::xml::chars::validate_name(target) || crate::xml::chars::is_reserved_pi_target(target)
    {
        return Err(MutError::BadName);
    }
    if !validate_chars(data) {
        return Err(MutError::BadChars);
    }
    if !value_seq_ok(ArenaKind::Pi, data) {
        return Err(MutError::BadChars);
    }
    let pi = doc.new_node(ArenaKind::Pi)?;
    let t = doc.store(target)?;
    let d = doc.store(data)?;
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
) -> Result<NodeId, MutError> {
    /* The parser's rules, not looser ones: a DOCTYPE the factory accepted but
     * the parser rejects made `to_xml` output that did not re-parse. The name
     * is a QName (not any Name: "a:b:c" and ":a" were accepted); a PUBLIC id is
     * PubidChar only; a SYSTEM id may hold either quote, since the writer picks
     * the other one, but not both, which no literal can hold. */
    if crate::xml::qname::split_checked(name).is_none() {
        return Err(MutError::BadName);
    }
    if pub_id.is_some_and(|id| !crate::xml::chars::is_pubid(id)) {
        return Err(MutError::BadChars);
    }
    if sys_id.is_some_and(|id| !validate_chars(id) || (id.contains(&b'"') && id.contains(&b'\''))) {
        return Err(MutError::BadChars);
    }
    doc.new_doctype(name, pub_id, sys_id)
        .map_err(MutError::from)
}

/// A detached, empty DOCUMENT_FRAGMENT.
///
/// It exists because the HTML-to-XML cross-import needs one and everything else
/// it builds already comes from this module; without it that path reached into
/// the arena's `new_node` directly, which is the layer the arena's `pub(super)`
/// now closes off.
pub fn new_fragment(doc: &mut Document) -> Result<NodeId, MutError> {
    doc.new_node(ArenaKind::DocumentFragment)
        .map_err(MutError::from)
}
