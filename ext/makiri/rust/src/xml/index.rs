//! Safe element-name index: `(local name, namespace URI)` -> document-ordered
//! elements.
//!
//! The cache is built and read through the index-arena [`Document`], so this
//! module contains no `unsafe` of its own. It owns no arena memory; its bucket
//! is a `&[NodeId]`, and a node id is exactly the opaque token the XPath engine
//! carries, so the FFI adapter can hand the slice to the engine without a
//! conversion.

#![forbid(unsafe_code)]

use crate::falloc;
use crate::falloc::{MapInsert, Reserve, VecPush};
use crate::xml::{Document, NodeId, NodeType};
use core::cell::Cell;
use core::hash::{BuildHasherDefault, Hasher};
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
    map: HashMap<Box<[u8]>, Vec<NodeId>, BuildHasherDefault<Fnv>>,
    /// The reusable key buffer, so the answer path does not allocate. A `Cell`,
    /// because the index is read through a shared borrow of its document; the
    /// GVL serialises lookups, and one takes the buffer and puts it back.
    scratch: Cell<Vec<u8>>,
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

fn build(doc: &Document) -> Option<Box<NameIndex>> {
    let root = doc.doc_node();
    let mut map: HashMap<Box<[u8]>, Vec<NodeId>, BuildHasherDefault<Fnv>> = HashMap::default();
    let mut key = Vec::new();
    let mut max_key = 0usize;
    let mut cur = Some(root);
    while let Some(node) = cur {
        if doc.type_(node) == Some(NodeType::Element) {
            if !key_into(&mut key, doc.local(node), doc.ns(node)) {
                return None;
            }
            max_key = max_key.max(key.len());
            match map.get_mut(&key[..]) {
                Some(nodes) => nodes.mkr_push(node).ok()?,
                None => {
                    let key = falloc::try_to_boxed_slice(&key)?;
                    let mut nodes = falloc::try_vec_with_capacity(1)?;
                    nodes.mkr_push(node).ok()?;
                    map.mkr_insert(key, nodes).ok()?;
                }
            }
        }
        cur = doc.preorder_next(root, node);
    }
    falloc::try_box(NameIndex {
        map,
        scratch: Cell::new(falloc::try_vec_with_capacity(max_key)?),
        max_key,
    })
    .ok()
}

/// Builds lazily. `None` means the caller must walk the tree, never that a
/// partially built index can answer a query.
pub fn get(doc: &Document) -> Option<&NameIndex> {
    if doc.name_index.get().is_none() {
        /* Kept only when the build succeeds, so an OOM is retried next time. */
        let _ = doc.name_index.set(build(doc)?);
    }
    doc.name_index.get().map(|idx| &**idx)
}

pub fn invalidate(doc: &mut Document) {
    doc.name_index.take();
}

/// The bucket for `(local, ns)`, in document order. The slice borrows the cache
/// and stays valid until the next mutation invalidates it; an over-long or
/// absent key yields an empty slice.
pub fn lookup<'a>(idx: &'a NameIndex, local: &[u8], ns: &[u8]) -> &'a [NodeId] {
    if local.len().saturating_add(1).saturating_add(ns.len()) > idx.max_key {
        return &[];
    }
    /* The buffer was reserved for the longest key, so these do not allocate. */
    let mut key = idx.scratch.take();
    key.clear();
    key.extend_from_slice(local);
    key.push(0xFF);
    key.extend_from_slice(ns);
    let nodes = match idx.map.get(&key[..]) {
        Some(nodes) => nodes.as_slice(),
        None => &[],
    };
    idx.scratch.set(key);
    nodes
}

pub fn xml_name_index_get(doc: &Document) -> Option<&NameIndex> {
    get(doc)
}

pub fn xml_name_index_invalidate(doc: &mut Document) {
    invalidate(doc)
}

pub fn xml_name_index_lookup<'a>(idx: &'a NameIndex, local: &[u8], ns_uri: &[u8]) -> &'a [NodeId] {
    lookup(idx, local, ns_uri)
}
