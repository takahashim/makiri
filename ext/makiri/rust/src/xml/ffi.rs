//! Rust-internal `mkr_xml_*` adapters.
//!
//! The C-era names remain while the Ruby glue is ported, but no C caller
//! remains. These functions consequently use Rust's ABI; they still turn raw
//! `(ptr, len)` inputs into slices, call the engine and fill out-parameters.

/* Every function here is a raw-pointer boundary. The former C declarations are
 * useful history, but Rust callers use these definitions directly. */
#![allow(clippy::missing_safety_doc)]
// These retain pointer-and-length entry shapes during the glue migration; a
// small argument struct would obscure the actual raw-pointer boundary.
#![allow(clippy::too_many_arguments)]

use crate::xml::arena;
use crate::xml::chars::{self, ExpandMode};
use crate::xml::index;
use crate::xml::mutate;
use crate::xml::qname;
use crate::xml::tree;
use crate::xml::{
    bytes, empty, node_qname, Doc, Limits, Node, QName, SpanBuf, ERR_INTERNAL, OK, T_ATTRIBUTE,
};
use core::ffi::c_char;
use core::ptr;

#[inline]
unsafe fn put<T>(p: *mut T, v: T) {
    if !p.is_null() {
        *p = v;
    }
}

/* ---- document / arena (mkr_xml_node.h) ---- */

pub unsafe fn mkr_xml_doc_new() -> *mut Doc {
    arena::doc_new()
}

pub unsafe fn mkr_xml_doc_destroy(doc: *mut Doc) {
    arena::doc_destroy(doc)
}

pub unsafe fn mkr_xml_doc_memsize(doc: *const Doc) -> usize {
    arena::doc_memsize(doc)
}

pub unsafe fn mkr_xml_arena_node(doc: *mut Doc, type_: u32) -> *mut Node {
    arena::arena_node(doc, type_)
}

pub unsafe fn mkr_xml_arena_bytes(doc: *mut Doc, src: *const c_char, len: u32) -> *const c_char {
    if len == 0 {
        return empty();
    }
    if src.is_null() {
        return ptr::null();
    }
    arena::arena_bytes(doc, bytes(src, len))
}

pub unsafe fn mkr_xml_arena_spanbuf(doc: *mut Doc, cap: usize) -> SpanBuf {
    arena::arena_spanbuf(doc, cap)
}

pub unsafe fn mkr_xml_node_xmlns_decl(
    a: *const Node,
    prefix: *mut *const c_char,
    plen: *mut u32,
    uri: *mut *const c_char,
    ulen: *mut u32,
) -> i32 {
    let qn = node_qname(a);
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
    put(
        uri,
        if (*a).value.is_null() {
            empty()
        } else {
            (*a).value
        },
    );
    put(ulen, (*a).value_len);
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

pub unsafe fn mkr_xml_preorder_next(root: *const Node, cur: *mut Node) -> *mut Node {
    arena::preorder_next(root, cur)
}

unsafe fn write_qname(name: *const c_char, len: u32, sp: &qname::Split, out: *mut QName) {
    let base = name as *const u8;
    *out = QName {
        qname: name,
        qname_len: len,
        prefix: name,
        prefix_len: sp.prefix_len,
        local: base.add(sp.local_off as usize) as *const c_char,
        local_len: sp.local_len,
    };
}

pub unsafe fn mkr_xml_qname_split(name: *const c_char, len: u32, out: *mut QName) -> i32 {
    match qname::split_checked(bytes(name, len)) {
        Some(sp) => {
            write_qname(name, len, &sp, out);
            0
        }
        None => -1,
    }
}

pub unsafe fn mkr_xml_split_scanned_qname(name: *const c_char, len: u32, out: *mut QName) -> i32 {
    match qname::split_scanned(bytes(name, len)) {
        Some(sp) => {
            write_qname(name, len, &sp, out);
            0
        }
        None => -1,
    }
}

pub unsafe fn mkr_xml_qname_assign(doc: *mut Doc, node: *mut Node, qn: *const QName) -> i32 {
    arena::qname_assign(doc, node, &*qn)
}

/* ---- self-tests ---- */

pub unsafe fn mkr_xml_node_selftest() -> i32 {
    crate::xml::selftest::node_selftest()
}

pub unsafe fn mkr_xml_parse_selftest() -> i32 {
    crate::xml::selftest::parse_selftest()
}

pub unsafe fn mkr_xml_mutate_selftest() -> i32 {
    crate::xml::selftest::mutate_selftest()
}

/* ---- parse (mkr_xml.h) ---- */

pub unsafe fn mkr_xml_parse(src: *const c_char, len: usize, status: *mut i32) -> *mut Doc {
    mkr_xml_parse_ex(src, len, ptr::null(), status)
}

pub unsafe fn mkr_xml_parse_ex(
    src: *const c_char,
    len: usize,
    limits: *const Limits,
    status: *mut i32,
) -> *mut Doc {
    let lim = if limits.is_null() {
        None
    } else {
        Some((*limits).max_bytes)
    };
    match tree::parse_ex_raw(src, len, lim) {
        Ok(doc) => {
            put(status, OK);
            doc
        }
        Err(st) => {
            put(status, st);
            ptr::null_mut()
        }
    }
}

pub unsafe fn mkr_xml_parse_fragment(
    doc: *mut Doc,
    src: *const c_char,
    len: usize,
    inherit_doc_ns: i32,
    status: *mut i32,
) -> *mut Node {
    match tree::parse_fragment_raw(doc, src, len, inherit_doc_ns != 0) {
        Ok(frag) => {
            put(status, OK);
            frag
        }
        Err(st) => {
            put(status, st);
            ptr::null_mut()
        }
    }
}

/* ---- character data ---- */

pub fn mkr_xml_is_char(c: u32) -> i32 {
    chars::is_char(c) as i32
}

pub unsafe fn mkr_xml_validate_chars(src: *const c_char, len: u32) -> i32 {
    if chars::validate_chars(bytes(src, len)) {
        0
    } else {
        -1
    }
}

pub fn mkr_xml_is_name_start(c: u32) -> i32 {
    chars::is_name_start(c) as i32
}

pub fn mkr_xml_is_name_char(c: u32) -> i32 {
    chars::is_name_char(c) as i32
}

pub unsafe fn mkr_xml_validate_name(src: *const c_char, len: u32) -> i32 {
    if chars::validate_name(bytes(src, len)) {
        0
    } else {
        -1
    }
}

pub unsafe fn mkr_xml_is_reserved_pi_target(s: *const c_char, len: u32) -> i32 {
    chars::is_reserved_pi_target(bytes(s, len)) as i32
}

pub unsafe fn mkr_xml_expand(
    doc: *mut Doc,
    src: *const c_char,
    len: u32,
    mode: u32,
    out_len: *mut u32,
    status: *mut i32,
) -> *const c_char {
    if out_len.is_null() || status.is_null() {
        return ptr::null();
    }
    if len == 0 {
        *out_len = 0;
        return empty();
    }
    if doc.is_null() || src.is_null() {
        *status = ERR_INTERNAL;
        return ptr::null();
    }
    let m = if mode == 0 {
        ExpandMode::Text
    } else {
        ExpandMode::Attr
    };
    match tree::expand_arena(doc, bytes(src, len), m) {
        Ok((p, n)) => {
            *out_len = n;
            p
        }
        Err(st) => {
            *status = st;
            ptr::null()
        }
    }
}

/* ---- mutation (mkr_xml_mutate.h) ---- */

pub unsafe fn mkr_xml_detach(node: *mut Node) {
    mutate::detach(node)
}

pub unsafe fn mkr_xml_remove(doc: *mut Doc, node: *mut Node) {
    mutate::remove(doc, node)
}

pub unsafe fn mkr_xml_replace_with_fragment(
    doc: *mut Doc,
    target: *mut Node,
    frag: *mut Node,
) -> i32 {
    mutate::replace_with_fragment(doc, target, frag)
}

pub unsafe fn mkr_xml_rename(
    doc: *mut Doc,
    node: *mut Node,
    name: *const c_char,
    nlen: u32,
) -> i32 {
    mutate::rename(doc, node, bytes(name, nlen))
}

pub unsafe fn mkr_xml_set_attribute(
    doc: *mut Doc,
    el: *mut Node,
    name: *const c_char,
    nlen: u32,
    val: *const c_char,
    vlen: u32,
    out: *mut *mut Node,
) -> i32 {
    mutate::set_attribute(doc, el, bytes(name, nlen), bytes(val, vlen), out)
}

pub unsafe fn mkr_xml_remove_attribute(el: *mut Node, name: *const c_char, nlen: u32) -> i32 {
    mutate::remove_attribute(el, bytes(name, nlen))
}

pub unsafe fn mkr_xml_set_attribute_ns(
    doc: *mut Doc,
    el: *mut Node,
    ns: *const c_char,
    nslen: u32,
    name: *const c_char,
    nlen: u32,
    val: *const c_char,
    vlen: u32,
    out: *mut *mut Node,
) -> i32 {
    mutate::set_attribute_ns(
        doc,
        el,
        bytes(ns, nslen),
        bytes(name, nlen),
        bytes(val, vlen),
        out,
    )
}

pub unsafe fn mkr_xml_remove_attribute_ns(
    el: *mut Node,
    ns: *const c_char,
    nslen: u32,
    local: *const c_char,
    llen: u32,
) -> i32 {
    mutate::remove_attribute_ns(el, bytes(ns, nslen), bytes(local, llen))
}

pub unsafe fn mkr_xml_set_content(
    doc: *mut Doc,
    node: *mut Node,
    text: *const c_char,
    tlen: u32,
) -> i32 {
    mutate::set_content(doc, node, bytes(text, tlen))
}

pub unsafe fn mkr_xml_new_element(
    doc: *mut Doc,
    name: *const c_char,
    nlen: u32,
    out: *mut *mut Node,
) -> i32 {
    mutate::new_element(doc, bytes(name, nlen), out)
}

pub unsafe fn mkr_xml_new_loose_dom_element(
    doc: *mut Doc,
    qn: *const QName,
    ns: *const c_char,
    nslen: u32,
    out: *mut *mut Node,
) -> i32 {
    mutate::new_loose_dom_element(doc, qn, bytes(ns, nslen), out)
}

pub unsafe fn mkr_xml_new_document_type(
    doc: *mut Doc,
    name: *const c_char,
    nlen: u32,
    pub_id: *const c_char,
    plen: u32,
    sys_id: *const c_char,
    slen: u32,
    out: *mut *mut Node,
) -> i32 {
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
    mutate::new_document_type(doc, bytes(name, nlen), p, s, out)
}

pub unsafe fn mkr_xml_new_chardata(
    doc: *mut Doc,
    type_: u8,
    text: *const c_char,
    tlen: u32,
    out: *mut *mut Node,
) -> i32 {
    mutate::new_chardata(doc, type_ as u32, bytes(text, tlen), out)
}

pub unsafe fn mkr_xml_new_pi(
    doc: *mut Doc,
    target: *const c_char,
    tlen: u32,
    data: *const c_char,
    dlen: u32,
    out: *mut *mut Node,
) -> i32 {
    mutate::new_pi(doc, bytes(target, tlen), bytes(data, dlen), out)
}

pub unsafe fn mkr_xml_import_subtree(doc: *mut Doc, src: *const Node, out: *mut *mut Node) -> i32 {
    mutate::import_subtree(doc, src, out)
}

pub unsafe fn mkr_xml_copy_node(
    doc: *mut Doc,
    src: *const Node,
    deep: i32,
    out: *mut *mut Node,
) -> i32 {
    mutate::copy_node(doc, src, deep != 0, out)
}

pub unsafe fn mkr_xml_clone_node(
    doc: *mut Doc,
    src: *const Node,
    deep: bool,
    out: *mut *mut Node,
) -> i32 {
    mutate::clone_node(doc, src, deep, out)
}

pub unsafe fn mkr_xml_insert_child(doc: *mut Doc, parent: *mut Node, node: *mut Node) -> i32 {
    mutate::insert_child(doc, parent, node)
}

pub unsafe fn mkr_xml_insert_before(doc: *mut Doc, r: *mut Node, node: *mut Node) -> i32 {
    mutate::insert_before(doc, r, node)
}

pub unsafe fn mkr_xml_insert_after(doc: *mut Doc, r: *mut Node, node: *mut Node) -> i32 {
    mutate::insert_after(doc, r, node)
}

pub unsafe fn mkr_xml_replace_node(doc: *mut Doc, r: *mut Node, node: *mut Node) -> i32 {
    mutate::replace_node(doc, r, node)
}

/* ---- element-name index (mkr_xml_index.h) ---- */

pub unsafe fn mkr_xml_name_index_get(doc: *mut Doc) -> *mut index::NameIndex {
    match doc.as_mut().and_then(index::get) {
        Some(idx) => idx as *mut index::NameIndex,
        None => ptr::null_mut(),
    }
}

pub unsafe fn mkr_xml_name_index_invalidate(doc: *mut Doc) {
    if let Some(doc) = doc.as_mut() {
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
) -> *const *mut Node {
    if !out_count.is_null() {
        *out_count = 0;
    }
    let Some(idx) = idx.cast_mut().as_mut() else {
        return ptr::null();
    };
    // SAFETY: this is the pointer-and-length FFI boundary.  Unlike `bytes`,
    // these lengths are `usize`, so retain their full range rather than
    // narrowing them to the XML layout's `u32` fields.
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
    let (nodes, count) = index::lookup(idx, local, ns_uri);
    if !out_count.is_null() {
        *out_count = count;
    }
    nodes
}

/* keep the attribute-type constant referenced so the import list mirrors the
 * C unit's (documentation aid for the symbol audit) */
const _: u32 = T_ATTRIBUTE;
