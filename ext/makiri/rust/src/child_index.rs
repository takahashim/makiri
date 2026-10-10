//! Counting and indexing a child list without building it - `child_count` /
//! `child_at` and their element-only twins - once for both representations.
//!
//! A representation describes one parent's child list through [`ChildWalk`]
//! (the XPath `Dom` / `dom_rules::Tree` pattern); this module decides where a
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
//! ([`ChildWalk::node_of`]) - for HTML, a raw pointer.
//!
//! Lexbor/Ruby-free and safe, like the engine.

#![forbid(unsafe_code)]

/// One parent's child list, as a representation reads it.
pub trait ChildWalk {
    type Node: Copy;
    /// The parent's token - with the [`ChildList`], the key the memo is
    /// stored under.
    fn parent_token(&self) -> usize;
    fn first(&self) -> Option<Self::Node>;
    fn last(&self) -> Option<Self::Node>;
    fn next(&self, n: Self::Node) -> Option<Self::Node>;
    fn prev(&self, n: Self::Node) -> Option<Self::Node>;
    fn is_element(&self, n: Self::Node) -> bool;
    fn token(&self, n: Self::Node) -> usize;
    /// The node behind a token this module recorded for this parent's list.
    /// By the memo's invariant it is still one of the parent's children.
    /// None only for a token no node has, which is never recorded.
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
    fn counts<W: ChildWalk>(self, w: &W, n: W::Node) -> bool {
        match self {
            ChildList::Nodes => true,
            ChildList::Elements => w.is_element(n),
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

/// The first node from `n` (inclusive) along `step` that is in `list`.
fn kept<W: ChildWalk>(
    w: &W,
    list: ChildList,
    mut n: Option<W::Node>,
    step: impl Fn(&W, W::Node) -> Option<W::Node>,
) -> Option<W::Node> {
    while let Some(x) = n {
        if list.counts(w, x) {
            return Some(x);
        }
        n = step(w, x);
    }
    None
}

/// `steps` nodes of `list` on from `n` along `step`.
fn walk<W: ChildWalk>(
    w: &W,
    list: ChildList,
    mut n: W::Node,
    steps: usize,
    step: impl Fn(&W, W::Node) -> Option<W::Node> + Copy,
) -> Option<W::Node> {
    for _ in 0..steps {
        n = kept(w, list, step(w, n), step)?;
    }
    Some(n)
}

fn key_of<W: ChildWalk>(w: &W, list: ChildList) -> ChildListKey {
    ChildListKey {
        parent: w.parent_token(),
        list,
    }
}

/// How many children are in the parent's `list`.
pub fn child_count<W: ChildWalk>(
    memo: &mut ChildPositionMemo,
    version: u64,
    w: &W,
    list: ChildList,
) -> usize {
    let key = key_of(w, list);
    let mut known = memo.lookup(version, key);
    if let Some(n) = known.count {
        return n;
    }
    let mut count = 0usize;
    let mut n = kept(w, list, w.first(), W::next);
    while let Some(x) = n {
        count += 1;
        n = kept(w, list, w.next(x), W::next);
    }
    known.count = Some(count);
    memo.record(version, key, known);
    count
}

/// The child at `index` in the parent's `list`, or None past the end.
pub fn child_at<W: ChildWalk>(
    memo: &mut ChildPositionMemo,
    version: u64,
    w: &W,
    list: ChildList,
    index: usize,
) -> Option<W::Node> {
    let key = key_of(w, list);
    let mut known = memo.lookup(version, key);
    if known.count.is_some_and(|n| index >= n) {
        return None;
    }
    let found = match choose_start(index, known) {
        Start::Front => {
            let first = kept(w, list, w.first(), W::next)?;
            walk(w, list, first, index, W::next)
        }
        Start::Back { steps } => {
            let last = kept(w, list, w.last(), W::prev)?;
            walk(w, list, last, steps, W::prev)
        }
        Start::Memo { from, token } => {
            let at = w.node_of(token)?;
            if index >= from {
                walk(w, list, at, index - from, W::next)
            } else {
                walk(w, list, at, from - index, W::prev)
            }
        }
    }?;
    known.at = Some((index, w.token(found)));
    memo.record(version, key, known);
    Some(found)
}

#[cfg(test)]
mod tests;
