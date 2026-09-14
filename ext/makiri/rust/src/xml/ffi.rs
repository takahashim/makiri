//! Rust-internal `mkr_xml_*` adapters.
//!
//! The C-era names remain for the glue, but no C caller remains and the node
//! handle is now an index-arena [`NodeId`], so these are Rust functions. They
//! turn raw `(ptr, len)` inputs into slices, call the engine, and fill
//! out-parameters. This is the only place a raw document pointer is dereferenced
//! for the XML tree.

#![allow(clippy::missing_safety_doc)]
#![allow(clippy::too_many_arguments)]

use crate::xml::index;
use crate::xml::mutate;
use crate::xml::qname;
use crate::xml::tree;
use crate::xml::{
    bytes, empty, Document, Limits, MutStatus, NodeId, NodeType, Span, Status, MAX_BYTES,
};
use core::ffi::c_char;
use core::ptr;

#[inline]
unsafe fn put<T>(p: *mut T, v: T) {
    if !p.is_null() {
        *p = v;
    }
}

/// Write a mutation result into an `out` handle: the node id on success, the
/// invalid id on failure (the C entry points' contract).
#[inline]
unsafe fn put_node(out: *mut NodeId, r: Result<NodeId, MutStatus>) -> MutStatus {
    match r {
        Ok(n) => {
            put(out, n);
            MutStatus::Ok
        }
        Err(st) => {
            put(out, NodeId::INVALID);
            st
        }
    }
}

#[inline]
unsafe fn doc_mut<'a>(doc: *mut Document) -> Option<&'a mut Document> {
    doc.as_mut()
}

#[inline]
unsafe fn doc_ref<'a>(doc: *const Document) -> Option<&'a Document> {
    doc.as_ref()
}

/* ---- document / arena ---- */

pub unsafe fn mkr_xml_doc_new() -> *mut Document {
    match Document::create(None, 0) {
        Ok(doc) => Box::into_raw(doc),
        Err(_) => ptr::null_mut(),
    }
}

pub unsafe fn mkr_xml_doc_destroy(doc: *mut Document) {
    if !doc.is_null() {
        drop(Box::from_raw(doc));
    }
}

pub unsafe fn mkr_xml_doc_memsize(doc: *const Document) -> usize {
    match doc_ref(doc) {
        Some(d) => d.memsize(),
        None => 0,
    }
}

pub unsafe fn mkr_xml_arena_node(doc: *mut Document, type_: NodeType) -> NodeId {
    match doc_mut(doc) {
        Some(d) => d.new_node(type_).unwrap_or(NodeId::INVALID),
        None => NodeId::INVALID,
    }
}

pub unsafe fn mkr_xml_arena_bytes(doc: *mut Document, src: *const c_char, len: u32) -> Span {
    match doc_mut(doc) {
        Some(d) => d.store(bytes(src, len)).unwrap_or(Span::EMPTY),
        None => Span::EMPTY,
    }
}

pub unsafe fn mkr_xml_node_xmlns_decl(
    doc: *const Document,
    a: NodeId,
    prefix: *mut *const c_char,
    plen: *mut u32,
    uri: *mut *const c_char,
    ulen: *mut u32,
) -> i32 {
    let Some(doc) = doc_ref(doc) else {
        return 0;
    };
    let qn = doc.qname(a);
    let p = match qname::xmlns_prefix(qn) {
        Some(p) => p,
        None => return 0,
    };
    put(
        prefix,
        if p.is_empty() {
            empty()
        } else {
            p.as_ptr() as *const c_char
        },
    );
    put(plen, p.len() as u32);
    let u = doc.value(a);
    put(
        uri,
        if u.is_empty() {
            empty()
        } else {
            u.as_ptr() as *const c_char
        },
    );
    put(ulen, u.len() as u32);
    1
}

pub unsafe fn mkr_xml_xmlns_prefix(
    name: *const c_char,
    len: u32,
    prefix: *mut *const c_char,
    plen: *mut u32,
) -> i32 {
    match qname::xmlns_prefix(bytes(name, len)) {
        Some(p) => {
            put(
                prefix,
                if p.is_empty() {
                    empty()
                } else {
                    p.as_ptr() as *const c_char
                },
            );
            put(plen, p.len() as u32);
            1
        }
        None => 0,
    }
}

pub unsafe fn mkr_xml_preorder_next(doc: *const Document, root: NodeId, cur: NodeId) -> NodeId {
    match doc_ref(doc).and_then(|d| d.preorder_next(root, cur)) {
        Some(n) => n,
        None => NodeId::INVALID,
    }
}

/* ---- self-tests (Ruby-only: the harness calls them through `__c_selftest`) ---- */

#[cfg(feature = "ruby")]
pub unsafe fn mkr_xml_node_selftest() -> i32 {
    crate::xml::selftest::node_selftest()
}

#[cfg(feature = "ruby")]
pub unsafe fn mkr_xml_parse_selftest() -> i32 {
    crate::xml::selftest::parse_selftest()
}

#[cfg(feature = "ruby")]
pub unsafe fn mkr_xml_mutate_selftest() -> i32 {
    crate::xml::selftest::mutate_selftest()
}

/* ---- parse ---- */

pub unsafe fn mkr_xml_parse(src: *const c_char, len: usize, status: *mut Status) -> *mut Document {
    mkr_xml_parse_ex(src, len, ptr::null(), status)
}

pub unsafe fn mkr_xml_parse_ex(
    src: *const c_char,
    len: usize,
    limits: *const Limits,
    status: *mut Status,
) -> *mut Document {
    let lim = limits.as_ref().map(|l| l.max_bytes);
    let max = lim.filter(|&n| n != 0).unwrap_or(MAX_BYTES);
    if len > max {
        put(status, Status::Limit);
        return ptr::null_mut();
    }
    let src = if src.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(src as *const u8, len)
    };
    match tree::parse_ex(src, lim) {
        Ok(doc) => {
            put(status, Status::Ok);
            doc
        }
        Err(st) => {
            put(status, st);
            ptr::null_mut()
        }
    }
}

pub unsafe fn mkr_xml_parse_fragment(
    doc: *mut Document,
    src: *const c_char,
    len: usize,
    inherit_doc_ns: bool,
    status: *mut Status,
) -> NodeId {
    let Some(doc) = doc_mut(doc) else {
        put(status, Status::Internal);
        return NodeId::INVALID;
    };
    if len > doc.max_bytes {
        put(status, Status::Limit);
        return NodeId::INVALID;
    }
    let src = if src.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(src as *const u8, len)
    };
    match tree::parse_fragment(doc, src, inherit_doc_ns) {
        Ok(frag) => {
            put(status, Status::Ok);
            frag
        }
        Err(st) => {
            put(status, st);
            NodeId::INVALID
        }
    }
}

/* ---- mutation (mkr_xml_mutate.h) ---- */

pub unsafe fn mkr_xml_detach(doc: *mut Document, node: NodeId) {
    if let (Some(doc), false) = (doc_mut(doc), node.is_invalid()) {
        mutate::detach(doc, node);
    }
}

pub unsafe fn mkr_xml_remove(doc: *mut Document, node: NodeId) {
    if let (Some(doc), false) = (doc_mut(doc), node.is_invalid()) {
        mutate::remove(doc, node);
    }
}

pub unsafe fn mkr_xml_replace_with_fragment(
    doc: *mut Document,
    target: NodeId,
    frag: NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        return MutStatus::Internal;
    };
    mutate::replace_with_fragment(doc, target, frag)
}

pub unsafe fn mkr_xml_rename(
    doc: *mut Document,
    node: NodeId,
    name: *const c_char,
    nlen: u32,
) -> MutStatus {
    match doc_mut(doc) {
        Some(doc) => mutate::rename(doc, node, bytes(name, nlen)),
        None => MutStatus::Internal,
    }
}

pub unsafe fn mkr_xml_set_attribute(
    doc: *mut Document,
    el: NodeId,
    name: *const c_char,
    nlen: u32,
    val: *const c_char,
    vlen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(
        out,
        mutate::set_attribute(doc, el, bytes(name, nlen), bytes(val, vlen)),
    )
}

pub unsafe fn mkr_xml_remove_attribute(
    doc: *mut Document,
    el: NodeId,
    name: *const c_char,
    nlen: u32,
) -> bool {
    match doc_mut(doc) {
        Some(doc) => mutate::remove_attribute(doc, el, bytes(name, nlen)),
        None => false,
    }
}

pub unsafe fn mkr_xml_set_attribute_ns(
    doc: *mut Document,
    el: NodeId,
    ns: *const c_char,
    nslen: u32,
    name: *const c_char,
    nlen: u32,
    val: *const c_char,
    vlen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(
        out,
        mutate::set_attribute_ns(
            doc,
            el,
            bytes(ns, nslen),
            bytes(name, nlen),
            bytes(val, vlen),
        ),
    )
}

pub unsafe fn mkr_xml_remove_attribute_ns(
    doc: *mut Document,
    el: NodeId,
    ns: *const c_char,
    nslen: u32,
    local: *const c_char,
    llen: u32,
) -> bool {
    match doc_mut(doc) {
        Some(doc) => mutate::remove_attribute_ns(doc, el, bytes(ns, nslen), bytes(local, llen)),
        None => false,
    }
}

pub unsafe fn mkr_xml_set_content(
    doc: *mut Document,
    node: NodeId,
    text: *const c_char,
    tlen: u32,
) -> MutStatus {
    match doc_mut(doc) {
        Some(doc) => mutate::set_content(doc, node, bytes(text, tlen)),
        None => MutStatus::Internal,
    }
}

pub unsafe fn mkr_xml_new_element(
    doc: *mut Document,
    name: *const c_char,
    nlen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(out, mutate::new_element(doc, bytes(name, nlen)))
}

pub unsafe fn mkr_xml_new_loose_dom_element(
    doc: *mut Document,
    name: *const c_char,
    nlen: u32,
    prefix_len: u32,
    local_off: u32,
    local_len: u32,
    ns: *const c_char,
    nslen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(
        out,
        mutate::new_loose_dom_element(
            doc,
            bytes(name, nlen),
            prefix_len,
            local_off,
            local_len,
            bytes(ns, nslen),
        ),
    )
}

pub unsafe fn mkr_xml_new_document_type(
    doc: *mut Document,
    name: *const c_char,
    nlen: u32,
    pub_id: *const c_char,
    plen: u32,
    sys_id: *const c_char,
    slen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    let p = if pub_id.is_null() {
        None
    } else {
        Some(bytes(pub_id, plen))
    };
    let s = if sys_id.is_null() {
        None
    } else {
        Some(bytes(sys_id, slen))
    };
    put_node(out, mutate::new_document_type(doc, bytes(name, nlen), p, s))
}

pub unsafe fn mkr_xml_new_chardata(
    doc: *mut Document,
    type_: NodeType,
    text: *const c_char,
    tlen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(out, mutate::new_chardata(doc, type_, bytes(text, tlen)))
}

pub unsafe fn mkr_xml_new_pi(
    doc: *mut Document,
    target: *const c_char,
    tlen: u32,
    data: *const c_char,
    dlen: u32,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(
        out,
        mutate::new_pi(doc, bytes(target, tlen), bytes(data, dlen)),
    )
}

pub unsafe fn mkr_xml_import_subtree(
    doc: *mut Document,
    src_doc: *const Document,
    src: NodeId,
    out: *mut NodeId,
) -> MutStatus {
    let (Some(doc), Some(src_doc)) = (doc_mut(doc), doc_ref(src_doc)) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(out, mutate::import_subtree(doc, src_doc, src))
}

pub unsafe fn mkr_xml_copy_node(
    doc: *mut Document,
    src_doc: *const Document,
    src: NodeId,
    deep: bool,
    out: *mut NodeId,
) -> MutStatus {
    let (Some(doc), Some(src_doc)) = (doc_mut(doc), doc_ref(src_doc)) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(out, mutate::copy_node_from(doc, src_doc, src, deep))
}

pub unsafe fn mkr_xml_clone_node(
    doc: *mut Document,
    src: NodeId,
    deep: bool,
    out: *mut NodeId,
) -> MutStatus {
    let Some(doc) = doc_mut(doc) else {
        put(out, NodeId::INVALID);
        return MutStatus::Internal;
    };
    put_node(out, mutate::clone_node(doc, src, deep))
}

pub unsafe fn mkr_xml_insert_child(doc: *mut Document, parent: NodeId, node: NodeId) -> MutStatus {
    match doc_mut(doc) {
        Some(doc) => mutate::insert_child(doc, parent, node),
        None => MutStatus::Internal,
    }
}

pub unsafe fn mkr_xml_insert_before(doc: *mut Document, r: NodeId, node: NodeId) -> MutStatus {
    match doc_mut(doc) {
        Some(doc) => mutate::insert_before(doc, r, node),
        None => MutStatus::Internal,
    }
}

pub unsafe fn mkr_xml_insert_after(doc: *mut Document, r: NodeId, node: NodeId) -> MutStatus {
    match doc_mut(doc) {
        Some(doc) => mutate::insert_after(doc, r, node),
        None => MutStatus::Internal,
    }
}

pub unsafe fn mkr_xml_replace_node(doc: *mut Document, r: NodeId, node: NodeId) -> MutStatus {
    match doc_mut(doc) {
        Some(doc) => mutate::replace_node(doc, r, node),
        None => MutStatus::Internal,
    }
}

/* ---- element-name index (mkr_xml_index.h) ---- */

pub unsafe fn mkr_xml_name_index_get(doc: *mut Document) -> *mut index::NameIndex {
    match doc_mut(doc).and_then(index::get) {
        Some(idx) => idx as *mut index::NameIndex,
        None => ptr::null_mut(),
    }
}

pub unsafe fn mkr_xml_name_index_invalidate(doc: *mut Document) {
    if let Some(doc) = doc_mut(doc) {
        index::invalidate(doc);
    }
}

pub unsafe fn mkr_xml_name_index_lookup(
    idx: *const index::NameIndex,
    local: *const c_char,
    local_len: usize,
    ns_uri: *const c_char,
    ns_uri_len: usize,
    out_count: *mut usize,
) -> *const *mut core::ffi::c_void {
    if !out_count.is_null() {
        *out_count = 0;
    }
    let Some(idx) = idx.cast_mut().as_mut() else {
        return ptr::null();
    };
    let local = if local.is_null() || local_len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(local as *const u8, local_len)
    };
    let ns_uri = if ns_uri.is_null() || ns_uri_len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(ns_uri as *const u8, ns_uri_len)
    };
    let nodes = index::lookup(idx, local, ns_uri);
    if !out_count.is_null() {
        *out_count = nodes.len();
    }
    // SAFETY: `NodeId` is one word and is exactly the opaque token the engine
    // carries, so the bucket slice is layout-identical to the engine's handle
    // buffer. The borrow lives until the next mutation invalidates the index.
    nodes.as_ptr() as *const *mut core::ffi::c_void
}

/* keep the attribute-type constant referenced so the import list mirrors the
 * C unit's (documentation aid for the symbol audit) */
const _: NodeType = NodeType::Attribute;
