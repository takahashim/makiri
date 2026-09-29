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
    /// `of S` ranks, one map per `of S` simple selector (its index), made
    /// on first use: a node's value is `rank << 1 | in_s`, `rank` being how
    /// many of it and the siblings before it (in the counting direction)
    /// are in `S`. Walking queries only, as `memo`.
    of_memo: Vec<(u32, PtrMap<*const (), u64>)>,
    /// The siblings `of S` counts have tested and not yet recorded, with
    /// whether each is in `S`, nearest first; a count's own run starts at
    /// its task's `seen_base`, as `Query::at` is shared by chains.
    of_seen: Vec<(*const (), bool)>,
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
            of_memo: Vec::new(),
            of_seen: Vec::new(),
        }
    }

    /// Where the next `of S` count's run in `of_seen` starts.
    pub(super) fn of_seen_base(&self) -> u32 {
        self.of_seen.len() as u32
    }

    /// Drop what a finished (or abandoned) count left in `of_seen`.
    pub(super) fn of_seen_truncate(&mut self, base: u32) {
        self.of_seen.truncate(base as usize);
    }

    /// Note that a count tested `node` and found it in `S` or not.
    pub(super) fn of_seen_push(
        &mut self,
        node: HtmlNode<'_>,
        in_s: bool,
    ) -> Result<(), MatchFailure> {
        self.of_seen.falloc_push((node_key(node), in_s)).or_oom()
    }

    /// `node`'s recorded `(rank, in_s)` for the `of S` selector `sel`.
    pub(super) fn of_rank(&self, sel: u32, node: HtmlNode<'_>) -> Option<(u64, bool)> {
        let (_, map) = self.of_memo.iter().find(|(k, _)| *k == sel)?;
        map.get(node_key(node)).map(|v| (v >> 1, v & 1 == 1))
    }

    /// Finish a count for `candidate` (in `S`): the run of tested siblings
    /// from `seen_base` on ends where the rank is `end_rank` (a known
    /// sibling's, or 0 at the end of the list). Returns the candidate's
    /// rank - its position among the siblings in `S` - and, in a walking
    /// query, records every sibling passed and the candidate itself.
    pub(super) fn of_finish(
        &mut self,
        sel: u32,
        seen_base: u32,
        candidate: HtmlNode<'_>,
        end_rank: u64,
    ) -> Result<u64, MatchFailure> {
        let run = self.of_seen.get(seen_base as usize..).unwrap_or_default();
        let members = run.iter().filter(|(_, in_s)| *in_s).count() as u64;
        let pos = end_rank + members + 1;
        if self.walking {
            let at = match self.of_memo.iter().position(|(k, _)| *k == sel) {
                Some(at) => at,
                None => {
                    self.of_memo.falloc_push((sel, PtrMap::new())).or_oom()?;
                    self.of_memo.len() - 1
                }
            };
            if let Some((_, map)) = self.of_memo.get_mut(at) {
                if map.len() + run.len() + 1 > POSITIONS_MAX {
                    *map = PtrMap::new();
                }
                // The farthest one passed sits next to `end_rank`.
                let mut rank = end_rank;
                for &(k, in_s) in run.iter().rev() {
                    rank += u64::from(in_s);
                    map.insert(k, rank << 1 | u64::from(in_s))
                        .map_err(|_| MatchFailure::Oom)?;
                }
                map.insert(node_key(candidate), pos << 1 | 1)
                    .map_err(|_| MatchFailure::Oom)?;
            }
        }
        self.of_seen.truncate(seen_base as usize);
        Ok(pos)
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
