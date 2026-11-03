//! Membership over a set's node words, for the set operators (`|`, `&`,
//! `-`): hashed above a size, scanned below it. Ruby-free.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use falloc::{Reserve, SetInsert};

use crate::bridge::wrapper::NodeWord;

/// Below this operand size a linear scan beats building a hash set.
const HASH_MIN: usize = 64;

type PtrBuild = core::hash::BuildHasherDefault<crate::ptr_table::MixHasher>;
type PtrSet = HashSet<NodeWord, PtrBuild>;

/// Membership over a node array: hashed above [`HASH_MIN`], scanned below it.
///
/// `try_reserve` rather than `reserve`, so an allocation failure degrades to
/// the linear scan - the C's "cap == 0 means not built" fallback - instead of
/// aborting the process.
pub(super) enum Index {
    Hashed(PtrSet),
    Linear,
}

impl Index {
    pub(super) fn build(nodes: &[NodeWord]) -> Index {
        if nodes.len() <= HASH_MIN {
            return Index::Linear;
        }
        let mut set = PtrSet::default();
        if set.falloc_reserve(nodes.len()).is_err() {
            return Index::Linear;
        }
        for &n in nodes {
            if set.falloc_insert(n).is_err() {
                return Index::Linear;
            }
        }
        Index::Hashed(set)
    }

    /// Sized for what is about to be inserted, holding nothing yet.
    pub(super) fn empty(expected: usize) -> Index {
        if expected <= HASH_MIN {
            return Index::Linear;
        }
        let mut set = PtrSet::default();
        if set.falloc_reserve(expected).is_err() {
            return Index::Linear;
        }
        Index::Hashed(set)
    }

    pub(super) fn contains(&self, n: NodeWord, fallback: &[NodeWord]) -> bool {
        match self {
            Index::Hashed(s) => s.contains(&n),
            Index::Linear => fallback.contains(&n),
        }
    }

    /// True when `n` had not been seen. `already` is what the linear fallback
    /// scans - the result built so far.
    pub(super) fn insert(&mut self, n: NodeWord, already: &[NodeWord]) -> bool {
        match self {
            /* Sized for every node it will see (`empty`), so this does not
             * grow; should it fail, the scan takes over for good, since
             * `already` is the whole answer so far. */
            Index::Hashed(s) => s.falloc_insert(n).unwrap_or_else(|()| {
                *self = Index::Linear;
                !already.contains(&n)
            }),
            Index::Linear => !already.contains(&n),
        }
    }
}
