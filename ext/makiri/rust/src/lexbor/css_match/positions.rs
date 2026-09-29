//! A node's position among its siblings for the `:nth-*` family, and the
//! memo that makes a walking query count each sibling list once.

use crate::falloc::{OomResult, VecPush};
use crate::lexbor::adapter::html::{HtmlNode, RawNode};
use crate::ptr_table::PtrMap;

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
}

/// A memo past this many entries is dropped and refilled, bounding it: the
/// positions are recounted on demand, so this costs time, never an answer.
const POSITIONS_MAX: usize = 1 << 16;

impl Positions {
    pub(super) fn new(walking: bool) -> Self {
        Positions {
            walking,
            memo: Vec::new(),
            visited: Vec::new(),
        }
    }
}

fn node_key(node: HtmlNode<'_>) -> *const () {
    RawNode::from(node).as_ptr().cast_const().cast()
}

/// `node`'s 1-based position among its (`from_end`-directed) siblings,
/// counting only same-type ones when `of_type`. Every sibling stepped over
/// charges `budget`.
///
/// With a memo ([`Positions`]): the walk in the counting direction stops at
/// the first counted sibling whose position is known (or at the end of the
/// list), and every counted sibling it passed gets its position recorded
/// too - so each is stepped over once per kind, not once per candidate.
pub(super) fn sibling_position<'doc>(
    node: HtmlNode<'doc>,
    from_end: bool,
    of_type: bool,
    budget: &Budget,
    positions: &mut Positions,
) -> Result<u64, MatchFailure> {
    // Element-only would agree for `of_type` (a non-element never satisfies
    // `name_matches_type`) but is WRONG for the plain (`of_type == false`)
    // case - see `counts_toward_child_position`'s doc. One walk serves both,
    // since `same_type` already answers `false` for a non-element itself.
    let same_type = |n: HtmlNode<'_>| !of_type || name_matches_type(n, node);
    let step = |n: HtmlNode<'doc>| {
        if from_end {
            next_position_sibling(n)
        } else {
            prev_position_sibling(n)
        }
    };
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
