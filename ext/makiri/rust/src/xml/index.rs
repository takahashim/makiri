//! Element-name index (mkr_xml_index.c): (local name, namespace URI) -> the
//! document-ordered elements bearing it. Lazily built, cached on the document,
//! dropped by `invalidate` from the single mutation hook.

use crate::falloc;
use crate::falloc::Reserve;
use crate::xml::arena::preorder_next;
use crate::xml::{node_local, node_ns, Doc, Node, T_ELEMENT};
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
    /// Scratch for `lookup`'s key, grown once at build time to the longest key
    /// the map holds, so a lookup NEVER allocates.
    ///
    /// This is not an optimisation, it is the correctness argument. `lookup`
    /// has no way to say "I could not answer": its caller reads a null return
    /// as the authoritative "no element bears this name" and stops, so an
    /// allocation failure here would become a silently empty node-set - a
    /// wrong answer, which is the one outcome the engine must never produce.
    /// (Measured: injecting that allocation made `count(//a)` return 0 and
    /// `normalize-space(//b)` return "".) An allocation that cannot happen
    /// cannot fail, so the branch is removed rather than handled.
    ///
    /// The bound is exact, not a guess: a requested key longer than every key
    /// in the map cannot match any of them, so `lookup` answers "no match"
    /// without needing the buffer at all.
    scratch: core::cell::UnsafeCell<Vec<u8>>,
    max_key: usize,
}

#[inline]
#[must_use]
fn key_into(buf: &mut Vec<u8>, local: &[u8], ns: &[u8]) -> bool {
    buf.clear();
    // The reserve is the only part that can fail, and one call covers the whole
    // key; after it the three writes cannot allocate.
    if buf.mkr_reserve(local.len() + 1 + ns.len()).is_err() {
        return false;
    }
    buf.extend_from_slice(local);
    buf.push(0xFF);
    buf.extend_from_slice(ns);
    true
}

unsafe fn build(doc: *mut Doc) -> *mut NameIndex {
    let root = (*doc).doc_node;
    if root.is_null() {
        return ptr::null_mut();
    }
    let mut map: HashMap<Box<[u8]>, Vec<*mut Node>, BuildHasherDefault<Fnv>> =
        HashMap::default();
    let mut key: Vec<u8> = Vec::new();
    let mut max_key = 0usize;
    let mut cur = root;
    while !cur.is_null() {
        if (*cur).type_ == T_ELEMENT {
            // Out of memory anywhere in the build abandons the whole index and
            // returns null. That is the fail-closed answer, not a degraded one:
            // `get` caches null as "not built" and every caller falls back to
            // walking the tree, which is the same answer this index exists to
            // make faster. A partially built index would be a WRONG answer -
            // `//tag` would miss the elements that did not get recorded.
            if !key_into(&mut key, node_local(cur), node_ns(cur)) {
                return ptr::null_mut();
            }
            if key.len() > max_key {
                max_key = key.len();
            }
            match map.get_mut(&key[..]) {
                Some(v) => {
                    if !falloc::try_push(v, cur) {
                        return ptr::null_mut();
                    }
                }
                None => {
                    let Some(k) = falloc::try_to_boxed_slice(&key) else {
                        return ptr::null_mut();
                    };
                    let Some(mut first) = falloc::try_vec_with_capacity(1) else {
                        return ptr::null_mut();
                    };
                    first.push(cur);
                    if !falloc::try_map_insert(&mut map, k, first) {
                        return ptr::null_mut();
                    }
                }
            }
        }
        cur = preorder_next(root, cur);
    }
    // One allocation for the lookup scratch, here where failure is already the
    // fail-closed "no index, walk instead" answer.
    let Some(scratch) = falloc::try_vec_with_capacity::<u8>(max_key) else {
        return ptr::null_mut();
    };
    falloc::try_box_raw(NameIndex {
        map,
        scratch: core::cell::UnsafeCell::new(scratch),
        max_key,
    })
}

/// The document's index, built and cached on first call (null on an empty
/// document; the caller then walks).
/// # Safety
/// `doc` must be a live document; the index borrows its nodes, so it is
/// invalidated by any mutation of that document.
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

/// # Safety
/// `doc` must be a live document; the index borrows its nodes, so it is
/// invalidated by any mutation of that document.
pub unsafe fn free(idx: *mut NameIndex) {
    if !idx.is_null() {
        drop(Box::from_raw(idx));
    }
}

/// # Safety
/// `doc` must be a live document; the index borrows its nodes, so it is
/// invalidated by any mutation of that document.
pub unsafe fn invalidate(doc: *mut Doc) {
    if doc.is_null() || (*doc).name_index.is_null() {
        return;
    }
    free((*doc).name_index as *mut NameIndex);
    (*doc).name_index = ptr::null_mut();
}

/// The document-ordered elements named (local, ns_uri): the borrowed bucket
/// and its count, or null / 0 on a miss.
/// # Safety
/// `doc` must be a live document; the index borrows its nodes, so it is
/// invalidated by any mutation of that document.
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
    // A key longer than any key in the map cannot match one, so answer the miss
    // directly - and, more to the point, without touching the scratch, whose
    // capacity is exactly `max_key`.
    if local.len() + 1 + ns.len() > (*idx).max_key {
        return ptr::null();
    }
    // SAFETY: the index is not shared across threads - every caller holds the
    // GVL - and the borrow ends inside this function, before returning. The
    // scratch was reserved for `max_key` bytes at build time and the write
    // above is bounded by it, so none of these three writes can allocate.
    let key = &mut *(*idx).scratch.get();
    key.clear();
    key.extend_from_slice(local);
    key.push(0xFF);
    key.extend_from_slice(ns);
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
