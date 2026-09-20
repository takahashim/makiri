//! Safe element-name index: `(local name, namespace URI)` -> document-ordered
//! elements.
//!
//! The cache is built and read through the index-arena [`Document`], so this
//! module contains no `unsafe` of its own. It owns no arena memory; its bucket
//! is a `&[NodeId]`, and a node id is exactly the opaque token the XPath engine
//! carries, so the XPath backend can hand the slice to the engine without a
//! conversion.

#![forbid(unsafe_code)]

use crate::falloc;
use crate::falloc::{MapInsert, Reserve, VecPush};
use crate::xml::{Document, NodeId, NodeType};
use core::cell::Cell;
use core::hash::{BuildHasherDefault, Hasher};
use std::collections::HashMap;

#[derive(Default)]
struct Fnv(u64);

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

type Map = HashMap<Box<[u8]>, Vec<NodeId>, BuildHasherDefault<Fnv>>;

/* ---- the key encoding, in one place ---- */

/// The bytes `(local, ns)` encodes to: `local ++ 0xFF ++ ns`. 0xFF is not valid
/// UTF-8, so it cannot occur inside either half and the encoding is injective.
fn key_len(local: &[u8], ns: &[u8]) -> usize {
    local.len().saturating_add(1).saturating_add(ns.len())
}

/// Write the key into `buf`, which must already hold [`key_len`] bytes of
/// capacity.
fn key_into(buf: &mut Vec<u8>, local: &[u8], ns: &[u8]) {
    buf.clear();
    buf.extend_from_slice(local);
    buf.push(0xFF);
    buf.extend_from_slice(ns);
}

/// The reusable key buffer, sized once for the longest key the map holds.
///
/// A [`Cell`], because the index is read through a shared borrow of its
/// document; the GVL serialises lookups, and one takes the buffer and puts it
/// back. [`KeyScratch::with_key`] is the only way in, so the take/put pair
/// cannot be split by an early return.
struct KeyScratch {
    buf: Cell<Vec<u8>>,
    /// The longest key in the map. A longer one cannot match anything, so it
    /// never reaches the buffer - which is what keeps this path allocation-free.
    max_key: usize,
}

impl KeyScratch {
    /// Run `f` on the encoded key, or answer `None` when no stored key can be
    /// that long.
    fn with_key<R>(&self, local: &[u8], ns: &[u8], f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        if key_len(local, ns) > self.max_key {
            return None;
        }
        /* Reserved for the longest key, so this does not allocate. */
        let mut buf = self.buf.take();
        key_into(&mut buf, local, ns);
        let r = f(&buf);
        self.buf.set(buf);
        Some(r)
    }
}

pub struct NameIndex {
    map: Map,
    scratch: KeyScratch,
}

impl NameIndex {
    /// The bucket for `(local, ns)`, in document order. The slice borrows the
    /// cache and stays valid until the next mutation invalidates it; an
    /// over-long or absent key yields an empty slice.
    pub fn lookup(&self, local: &[u8], ns: &[u8]) -> &[NodeId] {
        self.scratch
            .with_key(local, ns, |key| match self.map.get(key) {
                Some(nodes) => nodes.as_slice(),
                None => &[],
            })
            .unwrap_or(&[])
    }
}

fn build(doc: &Document) -> Option<Box<NameIndex>> {
    let root = doc.doc_node();
    let mut map = Map::default();
    let mut key = Vec::new();
    let mut max_key = 0usize;
    let mut cur = Some(root);
    while let Some(node) = cur {
        if doc.type_(node) == Some(NodeType::Element) {
            let (local, ns) = (doc.local(node), doc.ns(node));
            key.mkr_reserve(key_len(local, ns)).ok()?;
            key_into(&mut key, local, ns);
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
        scratch: KeyScratch {
            buf: Cell::new(falloc::try_vec_with_capacity(max_key)?),
            max_key,
        },
    })
    .ok()
}

impl Document {
    /// This document's element-name index, built lazily. `None` means the
    /// caller must walk the tree, never that a partially built index can answer
    /// a query.
    pub fn name_index(&self) -> Option<&NameIndex> {
        if self.name_index.get().is_none() {
            /* Kept only when the build succeeds, so an OOM is retried next time. */
            let _ = self.name_index.set(build(self)?);
        }
        self.name_index.get().map(|idx| &**idx)
    }

    /// Drop the index, which any mutation must do before changing links.
    pub fn invalidate_name_index(&mut self) {
        self.name_index.take();
    }
}
