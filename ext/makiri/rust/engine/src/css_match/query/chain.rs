//! A chain matched right to left ([`ChainTask`]): Lexbor's
//! `find` / `found_check` / `not_found` loop, with Blink's failure statuses
//! to keep its backtracking linear.

use crate::selectors::ast::Combinator;
use falloc::{OomResult, VecGrow};

use super::{Check, Outcome, Query};
use crate::css_match::compile::Chain;
use crate::css_match::dom::CssNode;
use crate::css_match::tree::{parent_element, prev_sibling_element};
use crate::css_match::MatchFailure;

/// Match `chain` right to left - Lexbor's `lxb_selectors_state_find` /
/// `found_check` / `not_found` over its entries: compound `idx` at `cur`,
/// its simple selectors from `rest` on. `at[at_base + i]` is where compound
/// `i` currently stands, which is what a backtrack moves on.
#[derive(Clone, Copy)]
pub(in crate::css_match) struct ChainTask<N> {
    pub(super) chain: Chain,
    pub(super) idx: u32,
    pub(super) cur: N,
    pub(super) rest: u32,
    pub(super) at_base: u32,
}

/// How far a failed left part of a chain is known to fail - Blink's
/// `SelectorChecker` match statuses. A `Descendant` step turns running out
/// of ancestors into `Completely`, a `SubsequentSibling` step running out of
/// siblings into `AllSiblings`; `>` and `+` pass either through.
#[derive(Clone, Copy)]
enum Fail {
    /// At this position only.
    Locally,
    /// At this position and every preceding sibling of it.
    AllSiblings,
    /// At this position, and at every ancestor and their preceding siblings.
    Completely,
}

impl<'c, 'p, 'd, N: CssNode<'d>> Query<'c, 'p, 'd, N> {
    /// A [`ChainTask`] matching `chain` (non-empty) at `node`.
    pub(super) fn chain_task(
        &mut self,
        chain: Chain,
        node: N,
    ) -> Result<ChainTask<N>, MatchFailure> {
        let at_base = u32::try_from(self.at.len()).map_err(|_| MatchFailure::Oom)?;
        // Compounds `1 .. len - 1`: a backtrack only ever comes back to
        // those (never to the first, never to the last).
        for _ in 2..chain.len() {
            self.at.falloc_push(None).or_oom()?;
        }
        let idx = chain.len - 1;
        Ok(ChainTask {
            chain,
            idx,
            cur: node,
            rest: self.compiled.compound(chain, idx as usize)?.start,
            at_base,
        })
    }

    fn set_at(&mut self, i: u32, node: N) {
        if let Some(slot) = self.at.get_mut(i as usize) {
            *slot = Some(node);
        }
    }

    /// [`ChainTask`]: a compound that matches moves left along its
    /// combinator; one that fails backtracks to the nearest choice at or
    /// right of it - a `Descendant` or `SubsequentSibling` combinator whose
    /// compound can move on to the next ancestor / preceding sibling. Every
    /// compound test charges the budget.
    ///
    /// Lexbor's `not_found` is exhaustive, which is exponential in the
    /// chain: `x div div div div div div p` over one `<p>` 40 deep tries
    /// every choice of six ancestors. A failure therefore carries how far it
    /// holds ([`Fail`], Blink's `SelectorChecker` statuses): a left part that
    /// failed at every ancestor fails at every ancestor of an ancestor too,
    /// so no move right of a `Descendant` can help, and likewise for every
    /// preceding sibling and a `SubsequentSibling`. Only moves that cannot
    /// change the answer are skipped - the verdict is Lexbor's.
    ///
    /// `at[at_base + i]` records where compound `i` last matched; it is read
    /// only when a backtrack comes back to `i` from the left, which only a
    /// match of `i` can have led to.
    pub(super) fn step_chain(
        &mut self,
        t: &mut ChainTask<N>,
        event: Option<bool>,
    ) -> Result<Outcome<N>, MatchFailure> {
        let compiled = self.compiled;
        let compounds = compiled
            .compounds
            .as_slice()
            .get(t.chain.start as usize..(t.chain.start + t.chain.len) as usize)
            .ok_or(MatchFailure::Internal)?;
        let compound = |i: u32| {
            compounds
                .get(i as usize)
                .copied()
                .ok_or(MatchFailure::Internal)
        };
        let last = t.chain.len - 1;
        let mut resumed = event;
        loop {
            let mut fail = Fail::Locally;
            let current = compound(t.idx)?;
            let matched = match resumed.take() {
                Some(false) => false,
                r => {
                    if r.is_none() {
                        self.budget.charge()?;
                    }
                    match self.check(current, t.rest, t.cur)? {
                        Check::Done(m) => m,
                        Check::Defer(i) => {
                            t.rest = i + 1;
                            let child = self.deferred_task(i, t.cur)?;
                            return Ok(Outcome::Spawn(child));
                        }
                    }
                }
            };
            if matched {
                if t.idx == 0 {
                    return Ok(Outcome::Done(true));
                }
                let next = match current.comb {
                    Combinator::Close | Combinator::Child | Combinator::Descendant => {
                        parent_element(t.cur)
                    }
                    Combinator::NextSibling | Combinator::SubsequentSibling => {
                        prev_sibling_element(t.cur)
                    }
                    // The column combinator `||`: `compile` refuses it
                    // before matching starts; this is the belt to that brace.
                    Combinator::Column | Combinator::Other => {
                        return Err(MatchFailure::Unsupported)
                    }
                };
                if let Some(n) = next {
                    if t.idx < last {
                        self.set_at(t.at_base + t.idx - 1, t.cur);
                    }
                    t.idx -= 1;
                    t.cur = n;
                    t.rest = compound(t.idx)?.start;
                    continue;
                }
                // Nowhere left to look: out of ancestors, or of siblings.
                fail = match current.comb {
                    Combinator::NextSibling | Combinator::SubsequentSibling => Fail::AllSiblings,
                    _ => Fail::Completely,
                };
            }
            // `not_found`: move the nearest choice at or right of `idx` on,
            // unless `fail` already holds for every place it could move to.
            loop {
                if t.idx >= last {
                    return Ok(Outcome::Done(false));
                }
                let moved = match (compound(t.idx + 1)?.comb, fail) {
                    (Combinator::Descendant, Fail::Locally | Fail::AllSiblings) => {
                        let n = parent_element(t.cur);
                        if n.is_none() {
                            fail = Fail::Completely;
                        }
                        n
                    }
                    (Combinator::SubsequentSibling, Fail::Locally) => {
                        let n = prev_sibling_element(t.cur);
                        if n.is_none() {
                            fail = Fail::AllSiblings;
                        }
                        n
                    }
                    // `>` and `+` have no choice to move; a failure that
                    // already holds everywhere passes through unchanged.
                    _ => None,
                };
                if let Some(n) = moved {
                    t.cur = n;
                    t.rest = compound(t.idx)?.start;
                    break;
                }
                t.idx += 1;
                if t.idx < last {
                    t.cur = self
                        .at
                        .get((t.at_base + t.idx - 1) as usize)
                        .copied()
                        .flatten()
                        .ok_or(MatchFailure::Internal)?;
                }
            }
        }
    }
}
