//! The child index against lists that implement [`ChildWalk`] and nothing
//! else: the answers against a direct read, the start choice, and the memo's
//! invariant (versions, eviction).

use super::*;
use crate::dom_rules::Tree;
use crate::node_type::NodeType;

/// One parent's children as a slice of "is an element" flags. Children are
/// their indices and the parent is [`PARENT`]; a child's token is its index
/// + 1 (a token of 0 names nothing), the parent's is `parent`. `steps`
/// counts sibling-link reads.
struct List<'a> {
    parent: usize,
    kinds: &'a [bool],
    steps: core::cell::Cell<usize>,
}

const PARENT: usize = usize::MAX;

impl<'a> List<'a> {
    fn new(parent: usize, kinds: &'a [bool]) -> Self {
        List {
            parent,
            kinds,
            steps: core::cell::Cell::new(0),
        }
    }

    fn step(&self, n: Option<usize>) -> Option<usize> {
        self.steps.set(self.steps.get() + 1);
        n
    }

    /// The direct answer: the children in `list`, in order.
    fn counted(&self, list: ChildList) -> Vec<usize> {
        (0..self.kinds.len())
            .filter(|&i| list == ChildList::Nodes || self.kinds[i])
            .collect()
    }
}

impl Tree for List<'_> {
    type Node = usize;
    fn node_type(&self, n: usize) -> NodeType {
        if n == PARENT || self.kinds[n] {
            NodeType::Element
        } else {
            NodeType::Text
        }
    }
    fn tree_parent(&self, n: usize) -> Option<usize> {
        (n != PARENT).then_some(PARENT)
    }
    fn host(&self, _n: usize) -> Option<usize> {
        None
    }
    fn first_child(&self, n: usize) -> Option<usize> {
        (n == PARENT && !self.kinds.is_empty()).then_some(0)
    }
    fn last_child(&self, n: usize) -> Option<usize> {
        if n == PARENT {
            self.kinds.len().checked_sub(1)
        } else {
            None
        }
    }
    fn next_sibling(&self, n: usize) -> Option<usize> {
        self.step((n + 1 < self.kinds.len()).then_some(n + 1))
    }
    fn prev_sibling(&self, n: usize) -> Option<usize> {
        self.step(n.checked_sub(1))
    }
}

impl TokenTree for List<'_> {
    fn token(&self, n: usize) -> usize {
        if n == PARENT {
            self.parent
        } else {
            n + 1
        }
    }
    fn node_of(&self, token: usize) -> Option<usize> {
        token.checked_sub(1)
    }
}

fn child_count(
    memo: &mut ChildPositionMemo,
    version: u64,
    tree: &List<'_>,
    list: ChildList,
) -> usize {
    Children {
        tree,
        parent: PARENT,
        list,
    }
    .count(memo, version)
}

fn child_at(
    memo: &mut ChildPositionMemo,
    version: u64,
    tree: &List<'_>,
    list: ChildList,
    index: usize,
) -> Option<usize> {
    Children {
        tree,
        parent: PARENT,
        list,
    }
    .at(memo, version, index)
}

/// Text, element, comment-ish (non-element), element, text, element.
const MIXED: [bool; 6] = [false, true, false, true, false, true];

#[test]
fn count_and_index_agree_with_a_direct_read() {
    for which in [ChildList::Nodes, ChildList::Elements] {
        let list = List::new(100, &MIXED);
        let want = list.counted(which);
        let mut memo = ChildPositionMemo::default();
        assert_eq!(child_count(&mut memo, 0, &list, which), want.len());
        /* Front, back, and memo starts, in a scrambled order. */
        for &i in &[3usize, 0, 5, 5, 1, 2, 4, 0, 3, 6, 9] {
            assert_eq!(
                child_at(&mut memo, 0, &list, which, i),
                want.get(i).copied(),
                "{which:?} index={i}"
            );
        }
    }
}

#[test]
fn an_empty_list_has_nothing_at_any_index() {
    let list = List::new(1, &[]);
    let mut memo = ChildPositionMemo::default();
    assert_eq!(child_at(&mut memo, 0, &list, ChildList::Nodes, 0), None);
    assert_eq!(child_count(&mut memo, 0, &list, ChildList::Nodes), 0);
    assert_eq!(child_at(&mut memo, 0, &list, ChildList::Nodes, 0), None);
    let none = List::new(2, &[false, false]);
    assert_eq!(child_count(&mut memo, 0, &none, ChildList::Elements), 0);
    assert_eq!(child_at(&mut memo, 0, &none, ChildList::Elements, 0), None);
}

#[test]
fn an_index_loop_walks_the_list_once() {
    let kinds = [false; 1000];
    let list = List::new(7, &kinds);
    let mut memo = ChildPositionMemo::default();
    let mut i = 0;
    while i < child_count(&mut memo, 0, &list, ChildList::Nodes) {
        assert_eq!(child_at(&mut memo, 0, &list, ChildList::Nodes, i), Some(i));
        i += 1;
    }
    /* One counting pass plus one step per index - not a walk per index. */
    assert!(
        list.steps.get() <= 2 * kinds.len(),
        "{} steps",
        list.steps.get()
    );
}

#[test]
fn the_start_is_the_nearest_known_point() {
    let empty = ChildListMemo::default();
    assert_eq!(choose_start(5, empty), Start::Front);
    let counted = ChildListMemo {
        count: Some(10),
        at: None,
    };
    assert_eq!(choose_start(2, counted), Start::Front);
    assert_eq!(choose_start(8, counted), Start::Back { steps: 1 });
    let near = ChildListMemo {
        count: Some(10),
        at: Some((6, 77)),
    };
    assert_eq!(choose_start(5, near), Start::Memo { from: 6, token: 77 });
    assert_eq!(choose_start(0, near), Start::Front);
    assert_eq!(choose_start(9, near), Start::Back { steps: 0 });
    /* Ties: the memo, then the back. Index 4 of 6 is one step from the
     * back and one from the memo at 3. */
    let tie = ChildListMemo {
        count: Some(6),
        at: Some((3, 9)),
    };
    assert_eq!(choose_start(4, tie), Start::Memo { from: 3, token: 9 });
    assert_eq!(
        choose_start(
            2,
            ChildListMemo {
                count: Some(5),
                at: None
            }
        ),
        Start::Back { steps: 2 }
    );
}

#[test]
fn another_version_finds_nothing_and_empties_the_memo() {
    let list = List::new(3, &MIXED);
    let key = ChildListKey {
        parent: 3,
        list: ChildList::Nodes,
    };
    let mut memo = ChildPositionMemo::default();
    child_count(&mut memo, 4, &list, ChildList::Nodes);
    child_at(&mut memo, 4, &list, ChildList::Nodes, 2);
    assert_eq!(
        memo.lookup(4, key),
        ChildListMemo {
            count: Some(6),
            at: Some((2, 3))
        }
    );
    assert_eq!(memo.lookup(5, key), ChildListMemo::default());
    /* A record under the new version drops every entry of the old one. */
    let other = ChildListKey {
        parent: 9,
        list: ChildList::Elements,
    };
    memo.record(5, other, ChildListMemo::default());
    assert_eq!(memo.lookup(4, key), ChildListMemo::default());
}

#[test]
fn the_two_lists_of_one_parent_are_kept_apart() {
    let list = List::new(3, &MIXED);
    let mut memo = ChildPositionMemo::default();
    assert_eq!(child_count(&mut memo, 0, &list, ChildList::Nodes), 6);
    assert_eq!(child_count(&mut memo, 0, &list, ChildList::Elements), 3);
    assert_eq!(
        child_at(&mut memo, 0, &list, ChildList::Elements, 1),
        Some(3)
    );
    assert_eq!(child_at(&mut memo, 0, &list, ChildList::Nodes, 1), Some(1));
    assert_eq!(child_count(&mut memo, 0, &list, ChildList::Nodes), 6);
}

#[test]
fn the_least_recently_used_list_is_evicted() {
    let mut memo = ChildPositionMemo::default();
    let key = |parent| ChildListKey {
        parent,
        list: ChildList::Nodes,
    };
    let known = |n| ChildListMemo {
        count: Some(n),
        at: None,
    };
    for p in 1..=CHILD_MEMO_ENTRIES {
        memo.record(0, key(p), known(p));
    }
    /* Touch the oldest, then one more list: the second oldest goes. */
    memo.record(0, key(1), known(1));
    memo.record(0, key(99), known(99));
    assert_eq!(memo.lookup(0, key(1)), known(1));
    assert_eq!(memo.lookup(0, key(2)), ChildListMemo::default());
    for p in 3..=CHILD_MEMO_ENTRIES {
        assert_eq!(memo.lookup(0, key(p)), known(p), "parent {p}");
    }
    assert_eq!(memo.lookup(0, key(99)), known(99));
}
