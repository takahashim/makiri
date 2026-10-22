//! A node's position among its siblings for the `:nth-*` family, and the
//! memo that makes a walking query count each sibling list once.

use crate::falloc::{OomResult, VecPush};
use crate::ptr_table::PtrMap;

use super::dom::CssNode;
use super::nth_of::{NthOf, NthOfMemo};
use super::scratch::{recycle, SCRATCH_KEEP};
use super::tree::{name_matches_type, next_position_sibling, prev_position_sibling};
use super::{Budget, MatchFailure};

/// Sibling positions a walking query has counted, so `:nth-child` and its
/// family over a wide sibling list costs O(siblings) per list rather than
/// O(siblings) per candidate - which, charged to the budget, made
/// `tr:nth-child(odd)` over ~4,500 rows raise. One memo per counting kind
/// (`from_end` x `of_type`), keyed by node; a walk records every counted
/// sibling it passes, so no list is counted twice (see [`sibling_position`]).
///
/// Only a walking query keeps one: a one-candidate `matches?` counts afresh,
/// as recording positions it never reads again would cost it more. Lexbor
/// keeps none (it recounts every time, with no budget to exhaust).
pub(super) struct Positions {
    walking: bool,
    /// One per kind, made on first use: a query that never counts
    /// positions - most - pays nothing for them, and `Query` stays small
    /// enough to move cheaply (a fixed array of four here measured ~5% on
    /// `matches?`).
    memo: Vec<PtrMap<*const (), u64>>,
    /// A walk's counted siblings, nearest first.
    visited: Vec<*const ()>,
    /// The `:nth-child(... of S)` ranks, which count by `S` rather than by
    /// position and are kept apart.
    nth_of: NthOfMemo,
}

/// A memo past this many entries is dropped and refilled, bounding it: the
/// positions are recounted on demand, so this costs time, never an answer.
pub(super) const POSITIONS_MAX: usize = 1 << 16;

/// A sibling position this close to the start of its list (in the counting
/// direction) is counted, not looked up ([`sibling_position`]).
const SHORT_LIST: u32 = 4;

/// A memo table kept in a [`Scratch`](super::Scratch) past this many slots
/// is dropped instead: emptying one costs its size, paid by the next query.
const KEEP_SLOTS: usize = 4 * SCRATCH_KEEP;

impl Positions {
    pub(super) const fn new() -> Self {
        Positions {
            walking: false,
            memo: Vec::new(),
            visited: Vec::new(),
            nth_of: NthOfMemo::new(),
        }
    }

    /// The `:nth-child(... of S)` memo, for a read.
    pub(super) fn nth_of_memo(&self) -> &NthOfMemo {
        &self.nth_of
    }

    /// The `:nth-child(... of S)` memo, recording only if this query walks.
    pub(super) fn nth_of(&mut self) -> NthOf<'_> {
        self.nth_of.with_walking(self.walking)
    }

    /// A query's positions, over the allocations `kept` holds (a
    /// [`Scratch`](super::Scratch)'s), so a warm `:nth-*` query makes no
    /// tables of its own.
    pub(super) fn reuse(kept: &mut Positions, walking: bool) -> Self {
        let mut p = core::mem::replace(kept, Positions::new());
        p.walking = walking;
        p
    }

    /// Hand the allocations back to `kept`, EMPTIED: a position holds only
    /// in the tree it was counted in, and after the query a node's address
    /// can belong to another node. Nothing counted outlives its query.
    pub(super) fn give_back(mut self, kept: &mut Positions) {
        for m in &mut self.memo {
            if m.capacity() > KEEP_SLOTS {
                *m = PtrMap::new();
            } else {
                m.clear();
            }
        }
        *kept = Positions {
            walking: false,
            memo: self.memo,
            visited: recycle(self.visited, SCRATCH_KEEP),
            nth_of: self.nth_of.emptied(),
        };
    }
}

pub(super) fn node_key<'d, N: CssNode<'d>>(node: N) -> *const () {
    node.table_key()
}

/// `node`'s 1-based position among its (`from_end`-directed) siblings,
/// counting only same-type ones when `of_type`. Every sibling stepped over
/// charges `budget`.
///
/// With a memo ([`Positions`]): the walk in the counting direction stops at
/// the first counted sibling whose position is known (or at the end of the
/// list), and every counted sibling it passed gets its position recorded
/// too - so each is stepped over once per kind, not once per candidate.
pub(super) fn sibling_position<'d, N: CssNode<'d>>(
    node: N,
    from_end: bool,
    of_type: bool,
    budget: &Budget,
    positions: &mut Positions,
) -> Result<u64, MatchFailure> {
    // Element-only would agree for `of_type` (a non-element never satisfies
    // `name_matches_type`) but is WRONG for the plain (`of_type == false`)
    // case - see `counts_toward_child_position`'s doc. One walk serves both,
    // since `same_type` already answers `false` for a non-element itself.
    let same_type = |n: N| !of_type || name_matches_type(n, node);
    let step = |n: N| {
        if from_end {
            next_position_sibling(n)
        } else {
            prev_position_sibling(n)
        }
    };
    // A short list is counted outright, as Lexbor does: cheaper than the
    // memo's lookups, and nothing to record.
    {
        let mut pos: u64 = 1;
        let mut cur = step(node);
        let mut left = SHORT_LIST;
        while let Some(n) = cur {
            if left == 0 {
                break;
            }
            left -= 1;
            budget.charge()?;
            if same_type(n) {
                pos += 1;
            }
            cur = step(n);
        }
        if cur.is_none() {
            return Ok(pos);
        }
    }
    let kind = usize::from(from_end) << 1 | usize::from(of_type);
    let walking = positions.walking;
    let Positions { memo, visited, .. } = positions;
    let memo = if walking {
        while memo.len() < 4 {
            memo.falloc_push(PtrMap::new()).or_oom()?;
        }
        memo.get_mut(kind)
    } else {
        None
    };
    let Some(memo) = memo else {
        let mut pos: u64 = 1;
        let mut cur = step(node);
        while let Some(n) = cur {
            budget.charge()?;
            if same_type(n) {
                pos += 1;
            }
            cur = step(n);
        }
        return Ok(pos);
    };
    if let Some(pos) = memo.get(node_key(node)) {
        return Ok(pos);
    }
    visited.clear();
    let mut base = 0;
    let mut cur = step(node);
    while let Some(n) = cur {
        budget.charge()?;
        if same_type(n) {
            if let Some(pos) = memo.get(node_key(n)) {
                base = pos;
                break;
            }
            visited.falloc_push(node_key(n)).or_oom()?;
        }
        cur = step(n);
    }
    if memo.len() + visited.len() + 1 > POSITIONS_MAX {
        *memo = PtrMap::new();
    }
    // The farthest one passed sits next to `base`; `node` after the nearest.
    let passed = visited.len() as u64;
    for (i, &k) in visited.iter().enumerate() {
        memo.insert(k, base + passed - i as u64)
            .map_err(|_| MatchFailure::Oom)?;
    }
    let pos = base + passed + 1;
    memo.insert(node_key(node), pos)
        .map_err(|_| MatchFailure::Oom)?;
    Ok(pos)
}
