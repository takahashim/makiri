//! The `:nth-child(... of S)` memo: each node's rank among the siblings in
//! `S`, and the nodes found not in `S`, kept for a walking query so a list is
//! counted once per `of S` selector - the `of S` half of what
//! [`Positions`](super::positions::Positions) does for plain positions.

use crate::falloc::{OomResult, VecGrow};
use crate::ptr_table::{NodeKey, PtrMap};

use super::dom::CssNode;
use super::positions::{node_key, POSITIONS_MAX};
use super::scratch::{recycle, SCRATCH_KEEP};
use super::MatchFailure;

pub(super) struct NthOfMemo {
    /// Ranks, one map per `of S` simple selector (its index), made on first
    /// use: a node's value is `rank << 1 | in_s`, `rank` being how many of it
    /// and the siblings before it (in the counting direction) are in `S`.
    ranks: Vec<(u32, PtrMap<NodeKey, u64>)>,
    /// The siblings `of S` counts have tested and not yet recorded, with
    /// whether each is in `S`, nearest first; a count's own run starts at
    /// its task's `seen_base`, as `Query::at` is shared by chains.
    seen: Vec<(NodeKey, bool)>,
    /// Candidates found NOT in `S`, one set per `of S` simple selector. Such
    /// a candidate's test ends its count, so it gets no rank in `ranks`,
    /// and without this every `:has()` that reaches it again tests `S`
    /// again - nested `of S` over nested `:has()` was 2.7x Lexbor's time.
    out: Vec<(u32, PtrMap<NodeKey, ()>)>,
}

impl NthOfMemo {
    pub(super) const fn new() -> Self {
        NthOfMemo {
            ranks: Vec::new(),
            seen: Vec::new(),
            out: Vec::new(),
        }
    }

    /// What a `Scratch` keeps for the next query: the `seen` buffer only.
    /// The maps are keyed by the query's own selector indexes, so they go.
    pub(super) fn emptied(self) -> Self {
        NthOfMemo {
            ranks: Vec::new(),
            seen: recycle(self.seen, SCRATCH_KEEP),
            out: Vec::new(),
        }
    }

    /// Where the next `of S` count's run in `seen` starts.
    pub(super) fn seen_base(&self) -> u32 {
        self.seen.len() as u32
    }

    /// The memo as a query reads and writes it: recording only when the
    /// query walks (`walking`, its [`Positions`](super::positions::Positions)'
    /// own flag - the one place it is kept).
    pub(super) fn with_walking(&mut self, walking: bool) -> NthOf<'_> {
        NthOf {
            memo: self,
            walking,
        }
    }
}

/// [`NthOfMemo`] lent to a query, with whether the query walks.
pub(super) struct NthOf<'a> {
    memo: &'a mut NthOfMemo,
    walking: bool,
}

impl NthOf<'_> {
    /// Drop what a finished (or abandoned) count left in `seen`.
    pub(super) fn truncate_seen(&mut self, base: u32) {
        self.memo.seen.truncate(base as usize);
    }

    /// Note that a count tested `node` and found it in `S` or not.
    pub(super) fn push_seen<'d, N: CssNode<'d>>(
        &mut self,
        node: N,
        in_s: bool,
    ) -> Result<(), MatchFailure> {
        self.memo.seen.falloc_push((node_key(node), in_s)).or_oom()
    }

    /// `node`'s recorded `(rank, in_s)` for the `of S` selector `sel`.
    pub(super) fn rank<'d, N: CssNode<'d>>(&self, sel: u32, node: N) -> Option<(u64, bool)> {
        let (_, map) = self.memo.ranks.iter().find(|(k, _)| *k == sel)?;
        map.get(node_key(node)).map(|v| (v >> 1, v & 1 == 1))
    }

    /// Is `node` known not to be in the `of S` selector `sel`'s `S`?
    pub(super) fn known_out<'d, N: CssNode<'d>>(&self, sel: u32, node: N) -> bool {
        self.memo
            .out
            .iter()
            .find(|(k, _)| *k == sel)
            .is_some_and(|(_, set)| set.get(node_key(node)).is_some())
    }

    /// Record that `node` is not in `sel`'s `S` (a walking query only).
    pub(super) fn note_out<'d, N: CssNode<'d>>(
        &mut self,
        sel: u32,
        node: N,
    ) -> Result<(), MatchFailure> {
        if !self.walking {
            return Ok(());
        }
        let at = match self.memo.out.iter().position(|(k, _)| *k == sel) {
            Some(at) => at,
            None => {
                self.memo.out.falloc_push((sel, PtrMap::new())).or_oom()?;
                self.memo.out.len() - 1
            }
        };
        if let Some((_, set)) = self.memo.out.get_mut(at) {
            if set.len() + 1 > POSITIONS_MAX {
                *set = PtrMap::new();
            }
            set.insert(node_key(node), ())
                .map_err(|_| MatchFailure::Oom)?;
        }
        Ok(())
    }

    /// Finish a count for `candidate` (in `S`): the run of tested siblings
    /// from `seen_base` on ends where the rank is `end_rank` (a known
    /// sibling's, or 0 at the end of the list). Returns the candidate's
    /// rank - its position among the siblings in `S` - and, in a walking
    /// query, records every sibling passed and the candidate itself.
    pub(super) fn finish<'d, N: CssNode<'d>>(
        &mut self,
        sel: u32,
        seen_base: u32,
        candidate: N,
        end_rank: u64,
    ) -> Result<u64, MatchFailure> {
        let run = self.memo.seen.get(seen_base as usize..).unwrap_or_default();
        let members = run.iter().filter(|(_, in_s)| *in_s).count() as u64;
        let pos = end_rank + members + 1;
        if self.walking {
            let at = match self.memo.ranks.iter().position(|(k, _)| *k == sel) {
                Some(at) => at,
                None => {
                    self.memo.ranks.falloc_push((sel, PtrMap::new())).or_oom()?;
                    self.memo.ranks.len() - 1
                }
            };
            if let Some((_, map)) = self.memo.ranks.get_mut(at) {
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
        self.memo.seen.truncate(seen_base as usize);
        Ok(pos)
    }
}
