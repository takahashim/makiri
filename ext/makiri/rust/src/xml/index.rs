//! Safe element-name index: `(local name, namespace URI)` -> document-ordered
//! elements. Raw XML pointers are converted at `ffi.rs`; this cache owns no
//! memory from the arena and never dereferences a pointer itself.

#![forbid(unsafe_code)]

use crate::falloc;
use crate::falloc::{MapInsert, Reserve, VecPush};
use crate::xml::{node_local_ref, node_ns_ref, node_type, preorder_next_ref, Doc, Node, T_ELEMENT};
use core::hash::{BuildHasherDefault, Hasher};
use core::ptr::NonNull;
use std::collections::HashMap;

#[derive(Default)]
pub struct Fnv(u64);

impl Hasher for Fnv {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

/// The key is `local ++ 0xFF ++ ns_uri`: 0xFF is not valid UTF-8.
pub struct NameIndex {
    map: HashMap<Box<[u8]>, Vec<*mut Node>, BuildHasherDefault<Fnv>>,
    /// GVL serialises lookup, so this reusable key never races and avoids an
    /// allocation on the answer path.
    scratch: Vec<u8>,
    max_key: usize,
}

#[must_use]
fn key_into(buf: &mut Vec<u8>, local: &[u8], ns: &[u8]) -> bool {
    buf.clear();
    if buf.mkr_reserve(local.len() + 1 + ns.len()).is_err() {
        return false;
    }
    buf.extend_from_slice(local);
    buf.push(0xFF);
    buf.extend_from_slice(ns);
    true
}

fn build(doc: &Doc) -> Option<Box<NameIndex>> {
    let root = NonNull::new(doc.doc_node)?;
    let mut map: HashMap<Box<[u8]>, Vec<*mut Node>, BuildHasherDefault<Fnv>> = HashMap::default();
    let mut key = Vec::new();
    let mut max_key = 0usize;
    let mut cur = Some(root);
    while let Some(node) = cur {
        if node_type(node) == T_ELEMENT {
            if !key_into(&mut key, node_local_ref(node), node_ns_ref(node)) {
                return None;
            }
            max_key = max_key.max(key.len());
            match map.get_mut(&key[..]) {
                Some(nodes) => nodes.mkr_push(node.as_ptr()).ok()?,
                None => {
                    let key = falloc::try_to_boxed_slice(&key)?;
                    let mut nodes = falloc::try_vec_with_capacity(1)?;
                    nodes.mkr_push(node.as_ptr()).ok()?;
                    map.mkr_insert(key, nodes).ok()?;
                }
            }
        }
        cur = preorder_next_ref(root, node);
    }
    falloc::try_box(NameIndex {
        map,
        scratch: falloc::try_vec_with_capacity(max_key)?,
        max_key,
    })
    .ok()
}

/// Builds lazily. `None` means the caller must walk the tree, never that a
/// partially built index can answer a query.
pub fn get(doc: &mut Doc) -> Option<&mut NameIndex> {
    if doc.name_index.is_none() {
        doc.name_index = build(doc);
    }
    doc.name_index.as_deref_mut()
}

pub fn invalidate(doc: &mut Doc) {
    doc.name_index = None;
}

/// Returns the bucket's raw pointer and count. The pointer is borrowed from
/// the cache and remains valid until the next mutation invalidates it.
pub fn lookup(idx: &mut NameIndex, local: &[u8], ns: &[u8]) -> (*const *mut Node, usize) {
    if local.len().saturating_add(1).saturating_add(ns.len()) > idx.max_key {
        return (core::ptr::null(), 0);
    }
    idx.scratch.clear();
    idx.scratch.extend_from_slice(local);
    idx.scratch.push(0xFF);
    idx.scratch.extend_from_slice(ns);
    match idx.map.get(&idx.scratch[..]) {
        Some(nodes) => (nodes.as_ptr(), nodes.len()),
        None => (core::ptr::null(), 0),
    }
}
