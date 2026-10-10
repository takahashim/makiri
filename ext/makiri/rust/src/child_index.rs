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
//! # Its known limit
//!
//! The version is the whole document's, so ANY child-list edit empties the
//! memo, not just one to a list it holds. A loop that edits one list while
//! indexing another (a diff applied as it is computed) re-walks from the
//! nearest end after every edit: O(min(i, n - i)) a step rather than O(1).
//! Dropping only the edited lists would need every edit to report the
//! parents it changed; `record_edit` knows only that a child list changed.
//!
//! Lexbor/Ruby-free and safe, like the engine.

#![forbid(unsafe_code)]

use crate::dom_rules::Tree;
use crate::node_type::NodeType;
use crate::ptr_table::{mix64, PtrMap, TableKey};

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

    /// The first node from `n` (inclusive) along `step` that is in this
    /// list. `step` is a generic, not a `fn` pointer, so each walk inlines its
    /// link read.
    #[inline]
    fn first_from<T: Tree>(
        self,
        tree: &T,
        mut n: Option<T::Node>,
        step: impl Fn(&T, T::Node) -> Option<T::Node>,
    ) -> Option<T::Node> {
        while let Some(x) = n {
            if self.counts(tree, x) {
                return Some(x);
            }
            n = step(tree, x);
        }
        None
    }
}

/* The DOM's element-only navigation - `firstElementChild`,
 * `lastElementChild`, `nextElementSibling`, `previousElementSibling` - over
 * the same links and the same element test as `child_at`'s `Elements` list,
 * so the readers cannot disagree with it. */

pub fn first_element_child<T: Tree>(tree: &T, n: T::Node) -> Option<T::Node> {
    ChildList::Elements.first_from(tree, tree.first_child(n), T::next_sibling)
}

pub fn last_element_child<T: Tree>(tree: &T, n: T::Node) -> Option<T::Node> {
    ChildList::Elements.first_from(tree, tree.last_child(n), T::prev_sibling)
}

pub fn next_element_sibling<T: Tree>(tree: &T, n: T::Node) -> Option<T::Node> {
    ChildList::Elements.first_from(tree, tree.next_sibling(n), T::next_sibling)
}

pub fn previous_element_sibling<T: Tree>(tree: &T, n: T::Node) -> Option<T::Node> {
    ChildList::Elements.first_from(tree, tree.prev_sibling(n), T::prev_sibling)
}

/// Which list of which parent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChildListKey {
    pub parent: usize,
    pub list: ChildList,
}

/// Keyed in a [`PtrMap`] by the parent's token, which is never 0 - an HTML
/// node pointer is not null, and XML arena slot 0 is reserved - so a 0
/// parent marks an empty slot.
impl TableKey for ChildListKey {
    const EMPTY: Self = ChildListKey {
        parent: 0,
        list: ChildList::Nodes,
    };
    #[inline]
    fn table_hash(self) -> u64 {
        /* Mixed first, so the list bit cannot cancel against a neighbouring
         * parent's low bit (XML tokens are consecutive slot indices). */
        mix64(self.parent as u64) ^ (self.list as u64)
    }
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

/// How many lists the memo holds before it starts over. Every list read
/// since the last child-list edit is kept - a DOM diff reads a list, then
/// every child's list under it, and must find the outer list's position when
/// it comes back - so this bounds only a long run of reads with no edit in
/// between (a full walk of a large document), where starting over costs one
/// re-walk per list still in use, once per this many lists. It also bounds
/// what a Document holds here outside the GC's view: ~230 KB at most.
pub const CHILD_MEMO_MAX: usize = 2_048;

/// A table bigger than this many slots is released, not cleared, when the
/// memo starts over: clearing writes every slot, which after one large walk
/// would make each later edit-then-read pay for the whole table.
const CHILD_MEMO_KEEP_SLOTS: usize = 256;

/// The document's remembered child lists, valid for one `tree_version` (the
/// module doc's invariant).
#[derive(Default)]
pub struct ChildPositionMemo {
    version: u64,
    lists: PtrMap<ChildListKey, ChildListMemo>,
}

impl ChildPositionMemo {
    /// What is known about `key`'s list at `version` - nothing, when the memo
    /// was filled under another version.
    pub fn lookup(&self, version: u64, key: ChildListKey) -> ChildListMemo {
        if self.version != version {
            return ChildListMemo::default();
        }
        self.lists.get(key).unwrap_or_default()
    }

    /// Remember `memo` for `key`'s list at `version`. A memo filled under
    /// another version is emptied first, and so is one at
    /// [`CHILD_MEMO_MAX`] about to take a NEW list - refining a list it
    /// holds evicts nothing. Out of memory, nothing is remembered: the memo
    /// only saves walks.
    pub fn record(&mut self, version: u64, key: ChildListKey, memo: ChildListMemo) {
        let full = self.lists.len() >= CHILD_MEMO_MAX && self.lists.get(key).is_none();
        if self.version != version || full {
            self.version = version;
            if self.lists.capacity() > CHILD_MEMO_KEEP_SLOTS {
                self.lists = PtrMap::new();
            } else {
                self.lists.clear();
            }
        }
        let _ = self.lists.set(key, memo);
    }

    /// The bytes the memo's table holds, for the Document's `memsize`.
    pub fn memsize(&self) -> usize {
        self.lists.capacity() * core::mem::size_of::<(ChildListKey, ChildListMemo)>()
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

    /// The first child in the list, or the last.
    fn first(&self) -> Option<T::Node> {
        self.list.first_from(
            self.tree,
            self.tree.first_child(self.parent),
            T::next_sibling,
        )
    }
    fn last(&self) -> Option<T::Node> {
        self.list.first_from(
            self.tree,
            self.tree.last_child(self.parent),
            T::prev_sibling,
        )
    }

    /// `steps` nodes of the list on from `n` along `step`: the node reached,
    /// or - when the list ends first - how many steps were taken and the
    /// last node, which tell a forward walk the list's length.
    fn walk(
        &self,
        mut n: T::Node,
        steps: usize,
        step: impl Fn(&T, T::Node) -> Option<T::Node> + Copy,
    ) -> Result<T::Node, (usize, T::Node)> {
        for taken in 0..steps {
            match self.list.first_from(self.tree, step(self.tree, n), step) {
                Some(next) => n = next,
                None => return Err((taken, n)),
            }
        }
        Ok(n)
    }

    /// How many children are in the list. A count that walks the list also
    /// remembers its last child, the position a reverse loop starts from.
    pub fn count(&self, memo: &mut ChildPositionMemo, version: u64) -> usize {
        let key = self.key();
        let mut known = memo.lookup(version, key);
        if let Some(n) = known.count {
            return n;
        }
        let mut count = 0usize;
        let mut last = None;
        let mut n = self.first();
        while let Some(x) = n {
            count += 1;
            last = Some(x);
            n = self
                .list
                .first_from(self.tree, self.tree.next_sibling(x), T::next_sibling);
        }
        known.count = Some(count);
        if let Some(x) = last {
            known.at = Some((count - 1, self.tree.token(x)));
        }
        memo.record(version, key, known);
        count
    }

    /// The child at `index` in the list, or None past the end. A walk that
    /// runs off the end remembers the length it found, so the next probe
    /// past it is answered at once.
    pub fn at(&self, memo: &mut ChildPositionMemo, version: u64, index: usize) -> Option<T::Node> {
        let key = self.key();
        let mut known = memo.lookup(version, key);
        if known.count.is_some_and(|n| index >= n) {
            return None;
        }
        /* Where the walk starts: the index of its first node, and the node. */
        let (from, start) = match choose_start(index, known) {
            Start::Front => match self.first() {
                Some(first) => (0, first),
                None => {
                    known.count = Some(0);
                    memo.record(version, key, known);
                    return None;
                }
            },
            /* `steps` is `count - 1 - index`, so this starts at `count - 1`. */
            Start::Back { steps } => (index + steps, self.last()?),
            Start::Memo { from, token } => (from, self.tree.node_of(token)?),
        };
        let walked = if index >= from {
            self.walk(start, index - from, T::next_sibling)
        } else {
            self.walk(start, from - index, T::prev_sibling)
        };
        match walked {
            Ok(found) => {
                known.at = Some((index, self.tree.token(found)));
                memo.record(version, key, known);
                Some(found)
            }
            /* Only a forward walk runs off the end: the list is that long. */
            Err((taken, last)) => {
                if index >= from {
                    let end = from + taken;
                    known.count = Some(end + 1);
                    known.at = Some((end, self.tree.token(last)));
                    memo.record(version, key, known);
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
