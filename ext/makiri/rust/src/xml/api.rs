//! Rust-internal `mkr_xml_*` adapters.
//!
//! The C-era names remain for the glue, but no C caller remains and the node
//! handle is now an index-arena [`NodeId`], so these are Rust functions.
//! The former C-shaped names remain temporarily for the glue, but every API
//! here uses ordinary Rust ownership, references, slices, and Results.

#![allow(clippy::too_many_arguments)]

use crate::xml::index;
use crate::xml::mutate;
use crate::xml::tree;
use crate::xml::{Document, Limits, MutStatus, NodeId, NodeType, Status, MAX_BYTES};

/// Write a mutation result into an `out` handle: the node id on success, the
/// invalid id on failure (the C entry points' contract).
#[inline]
fn put_node(out: &mut NodeId, r: Result<NodeId, MutStatus>) -> MutStatus {
    match r {
        Ok(n) => {
            *out = n;
            MutStatus::Ok
        }
        Err(st) => {
            *out = NodeId::INVALID;
            st
        }
    }
}

/* ---- document / arena ---- */

pub fn mkr_xml_doc_new() -> Result<Box<Document>, Status> {
    Document::create(None, 0)
}

pub fn mkr_xml_doc_destroy(doc: Box<Document>) {
    drop(doc);
}

pub fn mkr_xml_doc_memsize(doc: &Document) -> usize {
    doc.memsize()
}

pub fn mkr_xml_preorder_next(doc: &Document, root: NodeId, cur: NodeId) -> NodeId {
    match doc.preorder_next(root, cur) {
        Some(n) => n,
        None => NodeId::INVALID,
    }
}

/* ---- self-tests (Ruby-only: the harness calls them through `__c_selftest`) ---- */

#[cfg(feature = "ruby")]
pub fn mkr_xml_node_selftest() -> i32 {
    crate::xml::selftest::node_selftest()
}

#[cfg(feature = "ruby")]
pub fn mkr_xml_parse_selftest() -> i32 {
    crate::xml::selftest::parse_selftest()
}

#[cfg(feature = "ruby")]
pub fn mkr_xml_mutate_selftest() -> i32 {
    crate::xml::selftest::mutate_selftest()
}

/* ---- parse ---- */

pub fn mkr_xml_parse(src: &[u8]) -> Result<Box<Document>, Status> {
    mkr_xml_parse_ex(src, None)
}

pub fn mkr_xml_parse_ex(src: &[u8], limits: Option<&Limits>) -> Result<Box<Document>, Status> {
    let lim = limits.map(|l| l.max_bytes);
    let max = lim.filter(|&n| n != 0).unwrap_or(MAX_BYTES);
    if src.len() > max {
        return Err(Status::Limit);
    }
    tree::parse_ex(src, lim)
}

pub fn mkr_xml_parse_fragment(
    doc: &mut Document,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<NodeId, Status> {
    if src.len() > doc.max_bytes {
        return Err(Status::Limit);
    }
    tree::parse_fragment(doc, src, inherit_doc_ns)
}

/* ---- mutation (mkr_xml_mutate.h) ---- */

pub fn mkr_xml_detach(doc: &mut Document, node: NodeId) {
    if !node.is_invalid() {
        mutate::detach(doc, node);
    }
}

pub fn mkr_xml_remove(doc: &mut Document, node: NodeId) {
    if !node.is_invalid() {
        mutate::remove(doc, node);
    }
}

pub fn mkr_xml_replace_with_fragment(
    doc: &mut Document,
    target: NodeId,
    frag: NodeId,
) -> MutStatus {
    mutate::replace_with_fragment(doc, target, frag)
}

pub fn mkr_xml_rename(doc: &mut Document, node: NodeId, name: &[u8]) -> MutStatus {
    mutate::rename(doc, node, name)
}

pub fn mkr_xml_set_attribute(
    doc: &mut Document,
    el: NodeId,
    name: &[u8],
    val: &[u8],
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::set_attribute(doc, el, name, val))
}

pub fn mkr_xml_remove_attribute(doc: &mut Document, el: NodeId, name: &[u8]) -> bool {
    mutate::remove_attribute(doc, el, name)
}

pub fn mkr_xml_set_attribute_ns(
    doc: &mut Document,
    el: NodeId,
    ns: &[u8],
    name: &[u8],
    val: &[u8],
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::set_attribute_ns(doc, el, ns, name, val))
}

pub fn mkr_xml_remove_attribute_ns(
    doc: &mut Document,
    el: NodeId,
    ns: &[u8],
    local: &[u8],
) -> bool {
    mutate::remove_attribute_ns(doc, el, ns, local)
}

pub fn mkr_xml_set_content(doc: &mut Document, node: NodeId, text: &[u8]) -> MutStatus {
    mutate::set_content(doc, node, text)
}

pub fn mkr_xml_new_element(doc: &mut Document, name: &[u8], out: &mut NodeId) -> MutStatus {
    put_node(out, mutate::new_element(doc, name))
}

pub fn mkr_xml_new_loose_dom_element(
    doc: &mut Document,
    name: &[u8],
    prefix_len: u32,
    local_off: u32,
    local_len: u32,
    ns: &[u8],
    out: &mut NodeId,
) -> MutStatus {
    put_node(
        out,
        mutate::new_loose_dom_element(doc, name, prefix_len, local_off, local_len, ns),
    )
}

pub fn mkr_xml_new_document_type(
    doc: &mut Document,
    name: &[u8],
    pub_id: Option<&[u8]>,
    sys_id: Option<&[u8]>,
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::new_document_type(doc, name, pub_id, sys_id))
}

pub fn mkr_xml_new_chardata(
    doc: &mut Document,
    type_: NodeType,
    text: &[u8],
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::new_chardata(doc, type_, text))
}

pub fn mkr_xml_new_pi(
    doc: &mut Document,
    target: &[u8],
    data: &[u8],
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::new_pi(doc, target, data))
}

pub fn mkr_xml_import_subtree(
    doc: &mut Document,
    src_doc: &Document,
    src: NodeId,
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::import_subtree(doc, src_doc, src))
}

pub fn mkr_xml_copy_node(
    doc: &mut Document,
    src_doc: &Document,
    src: NodeId,
    deep: bool,
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::copy_node_from(doc, src_doc, src, deep))
}

pub fn mkr_xml_clone_node(
    doc: &mut Document,
    src: NodeId,
    deep: bool,
    out: &mut NodeId,
) -> MutStatus {
    put_node(out, mutate::clone_node(doc, src, deep))
}

pub fn mkr_xml_insert_child(doc: &mut Document, parent: NodeId, node: NodeId) -> MutStatus {
    mutate::insert_child(doc, parent, node)
}

pub fn mkr_xml_insert_before(doc: &mut Document, r: NodeId, node: NodeId) -> MutStatus {
    mutate::insert_before(doc, r, node)
}

pub fn mkr_xml_insert_after(doc: &mut Document, r: NodeId, node: NodeId) -> MutStatus {
    mutate::insert_after(doc, r, node)
}

pub fn mkr_xml_replace_node(doc: &mut Document, r: NodeId, node: NodeId) -> MutStatus {
    mutate::replace_node(doc, r, node)
}

/* ---- element-name index (mkr_xml_index.h) ---- */

pub fn mkr_xml_name_index_get(doc: &mut Document) -> Option<&mut index::NameIndex> {
    index::get(doc)
}

pub fn mkr_xml_name_index_invalidate(doc: &mut Document) {
    index::invalidate(doc);
}

pub fn mkr_xml_name_index_lookup<'a>(
    idx: &'a mut index::NameIndex,
    local: &[u8],
    ns_uri: &[u8],
) -> &'a [NodeId] {
    index::lookup(idx, local, ns_uri)
}

/* keep the attribute-type constant referenced so the import list mirrors the
 * C unit's (documentation aid for the symbol audit) */
const _: NodeType = NodeType::Attribute;
