//! Building a detached node: the `Document#create_*` family.
//!
//! Everything here comes back unattached and, for an element, with NO namespace
//! decided - it takes one from the context it is first inserted into
//! (`super::ns`). That is what makes building a subtree bottom-up and attaching
//! it give the same tree as building it top-down.

#![forbid(unsafe_code)]

use super::assign_qname;
use super::edit::dom_refuses_data;
use crate::xml::arena::DoctypeId;
use crate::xml::chars::validate_chars;
use crate::xml::qname::{split_checked, Split};
use crate::xml::{ArenaKind, Document, MutError, NodeFlags, NodeId, Span};

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

/// A detached element named `name` in `ns` ("" = none), its namespace decided
/// now - as the DOM's clone has its own from the moment it exists. For a copy
/// from another document (the HTML-to-XML import): [`new_element`]'s element
/// takes its namespace from where it is first inserted, which left an
/// imported `<p>` with none until then, where the DOM's is XHTML at once.
pub fn new_element_in(doc: &mut Document, name: &[u8], ns: &[u8]) -> Result<NodeId, MutError> {
    let span = stored_ns(doc, ns)?;
    new_element_in_span(doc, name, span)
}

/// [`new_element_in`] for a namespace URI already in `doc` at `ns`
/// ([`stored_ns`]), so an import that makes many elements in one namespace
/// stores it once. `ns` must be a span of `doc`'s, or empty.
pub fn new_element_in_span(doc: &mut Document, name: &[u8], ns: Span) -> Result<NodeId, MutError> {
    let el = new_element(doc, name)?;
    let n = doc.node_mut(el);
    n.ns_uri = ns;
    n.flags.insert(NodeFlags::NS_RESOLVED);
    Ok(el)
}

/// The span of namespace URI `ns` in `doc`, for the `_span` factories: for ""
/// (none) the absent span a new node starts with, else
/// [`Document::store_ns_uri`]'s.
pub fn stored_ns(doc: &mut Document, ns: &[u8]) -> Result<Span, MutError> {
    if ns.is_empty() {
        Ok(Span::ABSENT)
    } else {
        Ok(doc.store_ns_uri(ns)?)
    }
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
    let span = stored_ns(doc, ns)?;
    new_loose_dom_element_span(doc, name, sp, span)
}

/// [`new_loose_dom_element`] for a namespace URI already in `doc` at `ns`, as
/// [`new_element_in_span`].
pub fn new_loose_dom_element_span(
    doc: &mut Document,
    name: &[u8],
    sp: Split,
    ns: Span,
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
    let n = doc.node_mut(el);
    n.ns_uri = ns;
    n.flags.insert(NodeFlags::DOM_LOOSE_NAME);
    Ok(el)
}

/// The DOM's `createElementNS` element, its name already held to the DOM's
/// rule and split by it (`sp`), in `ns` ("" = none) decided now. A name that is
/// also an XML QName, split the same way, is an ordinary element
/// ([`new_element_in`]) that `to_xml` writes; any other is DOM-loose
/// ([`new_loose_dom_element`]), as the DOM allows and XML cannot write.
pub fn new_dom_element_ns(
    doc: &mut Document,
    name: &[u8],
    sp: Split,
    ns: &[u8],
) -> Result<NodeId, MutError> {
    let xml_name = split_checked(name)
        .is_some_and(|x| x == sp && !(x.prefix_len == 5 && name.starts_with(b"xmlns")));
    if xml_name {
        new_element_in(doc, name, ns)
    } else {
        new_loose_dom_element(doc, name, sp, ns)
    }
}

pub fn new_chardata(doc: &mut Document, ty: ArenaKind, text: &[u8]) -> Result<NodeId, MutError> {
    if ty != ArenaKind::Text && ty != ArenaKind::CDataSection && ty != ArenaKind::Comment {
        return Err(MutError::Type);
    }
    if let Some(why) = dom_refuses_data(ty, text) {
        return Err(MutError::InvalidCharacter(why));
    }
    let n = doc.new_node(ty)?;
    doc.set_value_bytes(n, text)?;
    Ok(n)
}

/// The DOM's `createProcessingInstruction`: `target` must be an XML Name and
/// `data` must not hold `?>`. A target the DOM takes and XML reserves (`xml`
/// in any case) is made, as `create_document_type` makes a doctype XML cannot
/// write, and the serializers refuse it.
pub fn new_pi(doc: &mut Document, target: &[u8], data: &[u8]) -> Result<NodeId, MutError> {
    if !crate::xml::chars::validate_name(target) {
        return Err(MutError::BadName);
    }
    if let Some(why) = dom_refuses_data(ArenaKind::Pi, data) {
        return Err(MutError::InvalidCharacter(why));
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

/// A detached DOCTYPE, the DOM's `createDocumentType`: the name is held to the
/// DOM's "valid doctype name" and the ids to nothing.
///
/// One XML cannot write is still made, and marked DOM-loose, so `to_xml`
/// refuses it (the DOM leaves that to a serializer asked for well-formed
/// output): the parser's rules decide which. The name is a QName (not any
/// Name: "a:b:c" and ":a" do not re-parse); a PUBLIC id is PubidChar only; a
/// SYSTEM id may hold either quote, since the writer picks the other one, but
/// not both, which no literal can hold. Refused outright, as they were, these
/// kept a browser's `createDocument(null, null, doctype)` from getting its
/// doctype at all (WPT `dom/common.js` makes one with the id `x"'y`).
pub fn new_document_type(
    doc: &mut Document,
    name: &[u8],
    pub_id: DoctypeId<'_>,
    sys_id: DoctypeId<'_>,
) -> Result<NodeId, MutError> {
    if !crate::xml::dom_name::valid_doctype_name(name) {
        return Err(MutError::BadDomName("invalid doctype name"));
    }
    let (public, system) = (pub_id.bytes(), sys_id.bytes());
    let writable = split_checked(name).is_some()
        && (!pub_id.is_written() || crate::xml::chars::is_pubid(public))
        && (!sys_id.is_written()
            || (validate_chars(system) && !(system.contains(&b'"') && system.contains(&b'\''))));
    let dt = doc.new_doctype(name, pub_id, sys_id)?;
    doc.node_mut(dt)
        .flags
        .set(NodeFlags::DOM_LOOSE_NAME, !writable);
    Ok(dt)
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
