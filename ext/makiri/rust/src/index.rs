//! Element-name index (mkr_xml_index.c): (local name, namespace URI) -> the
//! document-ordered elements bearing it. Lazily built, cached on the document,
//! dropped by `invalidate` from the single mutation hook.

use crate::arena::preorder_next;
use crate::{node_local, node_ns, Doc, Node, T_ELEMENT};
use core::ffi::{c_char, c_void};
use core::hash::{BuildHasherDefault, Hasher};
use core::ptr;
use std::collections::HashMap;

/// FNV-1a, matching the C index's hash (a cheap, dependency-free hasher).
#[derive(Default)]
pub struct Fnv(u64);

impl Hasher for Fnv {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

/// The key is `local ++ 0xFF ++ ns_uri`: 0xFF never occurs in valid UTF-8, so
/// the join is unambiguous ("ab"+"" != "a"+"b").
pub struct NameIndex {
    map: HashMap<Box<[u8]>, Vec<*mut Node>, BuildHasherDefault<Fnv>>,
}

#[inline]
fn key_into(buf: &mut Vec<u8>, local: &[u8], ns: &[u8]) {
    buf.clear();
    buf.extend_from_slice(local);
    buf.push(0xFF);
    buf.extend_from_slice(ns);
}

unsafe fn build(doc: *mut Doc) -> *mut NameIndex {
    let root = (*doc).doc_node;
    if root.is_null() {
        return ptr::null_mut();
    }
    let mut map: HashMap<Box<[u8]>, Vec<*mut Node>, BuildHasherDefault<Fnv>> =
        HashMap::default();
    let mut key: Vec<u8> = Vec::new();
    let mut cur = root;
    while !cur.is_null() {
        if (*cur).type_ == T_ELEMENT {
            key_into(&mut key, node_local(cur), node_ns(cur));
            match map.get_mut(&key[..]) {
                Some(v) => v.push(cur),
                None => {
                    map.insert(key.clone().into_boxed_slice(), vec![cur]);
                }
            }
        }
        cur = preorder_next(root, cur);
    }
    Box::into_raw(Box::new(NameIndex { map }))
}

/// The document's index, built and cached on first call (null on an empty
/// document; the caller then walks).
pub unsafe fn get(doc: *mut Doc) -> *mut NameIndex {
    if doc.is_null() {
        return ptr::null_mut();
    }
    if !(*doc).name_index.is_null() {
        return (*doc).name_index as *mut NameIndex;
    }
    let idx = build(doc);
    (*doc).name_index = idx as *mut c_void;
    idx
}

pub unsafe fn free(idx: *mut NameIndex) {
    if !idx.is_null() {
        drop(Box::from_raw(idx));
    }
}

pub unsafe fn invalidate(doc: *mut Doc) {
    if doc.is_null() || (*doc).name_index.is_null() {
        return;
    }
    free((*doc).name_index as *mut NameIndex);
    (*doc).name_index = ptr::null_mut();
}

/// The document-ordered elements named (local, ns_uri): the borrowed bucket
/// and its count, or null / 0 on a miss.
pub unsafe fn lookup(
    idx: *const NameIndex,
    local: *const c_char,
    local_len: usize,
    ns_uri: *const c_char,
    ns_uri_len: usize,
    out_count: *mut usize,
) -> *const *mut Node {
    if !out_count.is_null() {
        *out_count = 0;
    }
    if idx.is_null() || local.is_null() {
        return ptr::null();
    }
    let local = core::slice::from_raw_parts(local as *const u8, local_len);
    let ns: &[u8] = if ns_uri.is_null() || ns_uri_len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(ns_uri as *const u8, ns_uri_len)
    };
    let mut key = Vec::with_capacity(local.len() + 1 + ns.len());
    key_into(&mut key, local, ns);
    match (*idx).map.get(&key[..]) {
        Some(v) => {
            if !out_count.is_null() {
                *out_count = v.len();
            }
            v.as_ptr()
        }
        None => ptr::null(),
    }
}
