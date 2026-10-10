//! Counting and indexing a child list without building it - `child_count` /
//! `child_at` and their element-only twins - once for both representations.
//!
//! The child links are the ones the insertion rules read, [`Tree`]'s - one
//! description of each representation's tree, so `#children`, `child_at`
//! and the insertion rules cannot disagree about it. A representation adds
//! only what the memo needs ([`TokenTree`]); this module decides where a
//! walk starts and remembers where it ended, in a [`ChildPositionMemo`] the
//! document keeps. A loop that reads the count and then each index in turn
//! (`for (i = 0; i < n.childNodes.length; i++) n.childNodes[i]`) therefore
//! walks the list once in all, not once per index - the memo browsers keep
//! per NodeList.
//!
//! # The memo's one invariant
//!
//! A position the memo hands back names a node that is STILL the child it
//! was recorded as. [`ChildPositionMemo`] keeps that by itself: every
//! operation passes the document's `tree_version`, and a memo filled under
//! another version is emptied before it answers. The version moves before
//! every child-list edit changes anything (`bridge::wrapper::record_edit`),
//! so an entry that survives describes the tree as it is. This is what lets
//! a representation turn a recorded token back into a node
//! ([`TokenTree::node_of`]) - for HTML, a raw pointer.
//!
//! Lexbor/Ruby-free and safe, like the engine.

#![forbid(unsafe_code)]

use crate::dom_rules::Tree;
use crate::node_type::NodeType;

/// A [`Tree`] whose nodes the memo can record: a node as a token word and
/// back.
pub trait TokenTree: Tree {
    /// The node's token - for a parent, with the [`ChildList`], the key the
    /// memo is stored under.
    fn token(&self, n: Self::Node) -> usize;
    /// The node behind a token this module recorded. By the memo's invariant
    /// it is still a child of the parent it was recorded under. None only for
    /// a token no node has, which is never recorded.
    fn node_of(&self, token: usize) -> Option<Self::Node>;
}

/// Which of a parent's lists: DOM `childNodes` (every child) or `children`
/// (the elements only).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChildList {
    Nodes,
    Elements,
}

impl ChildList {
    /// Whether `n` is in this list.
    fn counts<T: Tree>(self, tree: &T, n: T::Node) -> bool {
        match self {
            ChildList::Nodes => true,
            ChildList::Elements => tree.node_type(n) == NodeType::Element,
        }
    }
}

/// Which list of which parent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChildListKey {
    pub parent: usize,
    pub list: ChildList,
}

/// What the memo knows about one list. Empty until a count or a position is
/// recorded.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ChildListMemo {
    /// How many children count, once counted.
    pub count: Option<usize>,
    /// The last position resolved: (index, node token).
    pub at: Option<(usize, usize)>,
}

/// How many lists are remembered. Several, not one: a DOM diff walks a list
/// while recursing into each child's list, and a single slot would let the
/// inner lists evict the outer one's count at every step. Sixteen covers that
/// recursion to that depth; it is a judgement, not a measurement.
pub const CHILD_MEMO_ENTRIES: usize = 16;

/// The document's remembered child lists, most recently used first, valid
/// for one `tree_version` (the module doc's invariant).
#[derive(Clone)]
pub struct ChildPositionMemo {
    version: u64,
    entries: [Option<(ChildListKey, ChildListMemo)>; CHILD_MEMO_ENTRIES],
}

impl Default for ChildPositionMemo {
    fn default() -> Self {
        ChildPositionMemo {
            version: 0,
            entries: [None; CHILD_MEMO_ENTRIES],
        }
    }
}

impl ChildPositionMemo {
    /// What is known about `key`'s list at `version` - nothing, when the memo
    /// was filled under another version.
    pub fn lookup(&self, version: u64, key: ChildListKey) -> ChildListMemo {
        if self.version != version {
            return ChildListMemo::default();
        }
        self.entries
            .iter()
            .flatten()
            .find(|(k, _)| *k == key)
            .map_or_else(ChildListMemo::default, |&(_, m)| m)
    }

    /// Remember `memo` for `key`'s list at `version`, as the most recently
    /// used; a memo filled under another version is emptied first.
    pub fn record(&mut self, version: u64, key: ChildListKey, memo: ChildListMemo) {
        if self.version != version {
            self.version = version;
            self.entries = [None; CHILD_MEMO_ENTRIES];
        }
        /* Move to the front: the key's old slot, or the least recently used. */
        let slot = self
            .entries
            .iter()
            .position(|e| e.is_some_and(|(k, _)| k == key))
            .unwrap_or(CHILD_MEMO_ENTRIES - 1);
        self.entries.copy_within(0..slot, 1);
        self.entries[0] = Some((key, memo));
    }
}

/// Where [`child_at`] starts its walk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Start {
    /// From the first counted child, `index` steps forward.
    Front,
    /// From the last counted child, `steps` back.
    Back { steps: usize },
    /// From the remembered position `from` (whose node is `token`).
    Memo { from: usize, token: usize },
}

/// The cheapest start for reaching `index`: the front costs `index` steps,
/// the back (with a known count) `count - 1 - index`, a remembered position
/// the distance to it. Ties go to the memo, then the back. The caller has
/// already answered an `index` at or past a known count.
pub fn choose_start(index: usize, known: ChildListMemo) -> Start {
    let mut best = (index, Start::Front);
    if let Some(n) = known.count {
        let steps = n.saturating_sub(1).saturating_sub(index);
        if steps <= best.0 {
            best = (steps, Start::Back { steps });
        }
    }
    if let Some((from, token)) = known.at {
        let distance = from.abs_diff(index);
        if distance <= best.0 {
            best = (distance, Start::Memo { from, token });
        }
    }
    best.1
}

/// One list of one parent: what `child_count` / `child_at` read.
pub struct Children<'t, T: TokenTree> {
    pub tree: &'t T,
    pub parent: T::Node,
    pub list: ChildList,
}

impl<T: TokenTree> Children<'_, T> {
    fn key(&self) -> ChildListKey {
        ChildListKey {
            parent: self.tree.token(self.parent),
            list: self.list,
        }
    }

    /// The first node from `n` (inclusive) along `step` that is in the list.
    fn kept(
        &self,
        mut n: Option<T::Node>,
        step: impl Fn(&T, T::Node) -> Option<T::Node>,
    ) -> Option<T::Node> {
        while let Some(x) = n {
            if self.list.counts(self.tree, x) {
                return Some(x);
            }
            n = step(self.tree, x);
        }
        None
    }

    /// `steps` nodes of the list on from `n` along `step`.
    fn walk(
        &self,
        mut n: T::Node,
        steps: usize,
        step: impl Fn(&T, T::Node) -> Option<T::Node> + Copy,
    ) -> Option<T::Node> {
        for _ in 0..steps {
            n = self.kept(step(self.tree, n), step)?;
        }
        Some(n)
    }

    /// How many children are in the list.
    pub fn count(&self, memo: &mut ChildPositionMemo, version: u64) -> usize {
        let key = self.key();
        let mut known = memo.lookup(version, key);
        if let Some(n) = known.count {
            return n;
        }
        let mut count = 0usize;
        let mut n = self.kept(self.tree.first_child(self.parent), T::next_sibling);
        while let Some(x) = n {
            count += 1;
            n = self.kept(self.tree.next_sibling(x), T::next_sibling);
        }
        known.count = Some(count);
        memo.record(version, key, known);
        count
    }

    /// The child at `index` in the list, or None past the end.
    pub fn at(&self, memo: &mut ChildPositionMemo, version: u64, index: usize) -> Option<T::Node> {
        let key = self.key();
        let mut known = memo.lookup(version, key);
        if known.count.is_some_and(|n| index >= n) {
            return None;
        }
        let found = match choose_start(index, known) {
            Start::Front => {
                let first = self.kept(self.tree.first_child(self.parent), T::next_sibling)?;
                self.walk(first, index, T::next_sibling)
            }
            Start::Back { steps } => {
                let last = self.kept(self.tree.last_child(self.parent), T::prev_sibling)?;
                self.walk(last, steps, T::prev_sibling)
            }
            Start::Memo { from, token } => {
                let at = self.tree.node_of(token)?;
                if index >= from {
                    self.walk(at, index - from, T::next_sibling)
                } else {
                    self.walk(at, from - index, T::prev_sibling)
                }
            }
        }?;
        known.at = Some((index, self.tree.token(found)));
        memo.record(version, key, known);
        Some(found)
    }
}

#[cfg(test)]
mod tests;
