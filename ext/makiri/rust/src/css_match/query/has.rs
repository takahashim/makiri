//! `:has()`'s forward search ([`ForwardTask`]), its candidates walked by a
//! resumable [`HasCursor`] per level, so no argument's search recurses
//! natively.

use crate::falloc::{OomResult, VecGrow};
use crate::selectors::ast::Combinator;

use super::{Check, Outcome, Query, Task};
use crate::css_match::compile::Chain;
use crate::css_match::dom::CssNode;
use crate::css_match::tree::next_sibling_element;
use crate::css_match::MatchFailure;

/// A resumable position in `:has()`'s forward search for ONE compound step -
/// the heap state a [`ForwardTask`] keeps per level so a `:has()` argument's
/// candidate search (potentially many candidates, unlike the main matcher's
/// single-path ancestor/sibling climbs) never recurses natively, whatever the
/// combinator (module doc). Built once per compound step ([`HasCursor::start`])
/// from the node the search starts FROM, then [`HasCursor::next`] repeatedly
/// for each candidate - same candidates, same order, same element-only
/// filtering as Lexbor's own forward search
/// (`lxb_selectors_state_found_check_forward`)/this port's earlier
/// native-recursive `has_forward`.
pub(in crate::css_match) enum HasCursor<N> {
    /// The combinator can never find anything (`Close` - defensive, per
    /// `Compiled::add_chain`: a compound boundary is always a real
    /// structural combinator, so this is not a real case in practice).
    Empty,
    /// `Combinator::Descendant`: the whole subtree, via
    /// [`HtmlNode::preorder_next`] directly (not [`HtmlNode::subtree`], which
    /// bundles the `.skip(1)` this cursor needs to resume mid-walk instead
    /// of re-deriving on every step).
    Subtree {
        current: N,
        root: N,
        /// The next step leaves `current`'s subtree out ([`HasCursor::prune`]).
        past: bool,
    },
    /// `Combinator::Child`: the next child to try.
    Child(Option<N>),
    /// `Combinator::NextSibling`: exactly one candidate, taken then spent.
    NextSibling(Option<N>),
    /// `Combinator::SubsequentSibling`: the next following sibling to try.
    SubsequentSibling(Option<N>),
}

impl<'d, N: CssNode<'d>> HasCursor<N> {
    /// Start searching for `comb`'s candidates from `from`. `Err(Unsupported)`
    /// for the column combinator `||`, exactly as the original `has_forward`
    /// raised it - Lexbor's own traversal cannot run it either.
    fn start(comb: Combinator, from: N) -> Result<Self, MatchFailure> {
        Ok(match comb {
            Combinator::Descendant => HasCursor::Subtree {
                current: from,
                root: from,
                past: false,
            },
            Combinator::Child => HasCursor::Child(from.first_child()),
            Combinator::NextSibling => HasCursor::NextSibling(next_sibling_element(from)),
            Combinator::SubsequentSibling => {
                HasCursor::SubsequentSibling(next_sibling_element(from))
            }
            Combinator::Close => HasCursor::Empty,
            Combinator::Column | Combinator::Other => return Err(MatchFailure::Unsupported),
        })
    }

    /// The next ELEMENT candidate, or `None` once the search is exhausted -
    /// [`Query::step_forward`] charges the budget for each one.
    fn next(&mut self) -> Option<N> {
        match self {
            HasCursor::Empty => None,
            HasCursor::Subtree {
                current,
                root,
                past,
            } => {
                let mut step = if core::mem::take(past) {
                    current.preorder_next_past(*root)
                } else {
                    current.preorder_next(*root)
                };
                while let Some(n) = step {
                    *current = n;
                    if n.element().is_some() {
                        return Some(n);
                    }
                    step = n.preorder_next(*root);
                }
                None
            }
            HasCursor::Child(cur) => loop {
                let n = (*cur)?;
                *cur = n.next();
                if n.element().is_some() {
                    return Some(n);
                }
            },
            HasCursor::NextSibling(cur) => cur.take(),
            HasCursor::SubsequentSibling(cur) => {
                let n = cur.take()?;
                *cur = next_sibling_element(n);
                Some(n)
            }
        }
    }
}

impl<N> HasCursor<N> {
    /// Leave the subtree of the candidate last returned out of the rest of
    /// the walk. Only a [`HasCursor::Subtree`] has candidates inside an
    /// earlier one's subtree; the others are left as they are.
    fn prune(&mut self) {
        if let HasCursor::Subtree { past, .. } = self {
            *past = true;
        }
    }
}

/// One `:has()` alternative's forward search from its anchor (Lexbor's
/// `*_forward` states): compound `level` tested at `cand`, from simple selector
/// `rest`; each level's candidates come from `cursors[cur_base + level]`.
#[derive(Clone, Copy)]
pub(in crate::css_match) struct ForwardTask<N> {
    pub(super) chain: Chain,
    pub(super) level: u32,
    pub(super) cand: N,
    pub(super) rest: u32,
    pub(super) cur_base: u32,
}

impl<'c, 'p, 'd, N: CssNode<'d>> Query<'c, 'p, 'd, N> {
    /// A [`ForwardTask`] searching for `chain` (non-empty) from `anchor`.
    pub(super) fn forward_task(
        &mut self,
        chain: Chain,
        anchor: N,
    ) -> Result<Task<N>, MatchFailure> {
        let cur_base = u32::try_from(self.cursors.len()).map_err(|_| MatchFailure::Oom)?;
        let cursor = HasCursor::start(self.compiled.compound(chain, 0)?.comb, anchor)?;
        self.cursors.falloc_push(cursor).or_oom()?;
        Ok(Task::Forward(ForwardTask {
            chain,
            level: 0,
            cand: anchor,
            rest: 0,
            cur_base,
        }))
    }

    /// [`ForwardTask`]: take the next candidate at the current level (or, out
    /// of them, back to the level before), test that level's compound on it,
    /// and on a match go one level deeper from it. The same candidates, in the
    /// same order, as Lexbor's forward search
    /// (`lxb_selectors_state_found_check_forward`).
    pub(super) fn step_forward(
        &mut self,
        t: &mut ForwardTask<N>,
        event: Option<bool>,
    ) -> Result<Outcome<N>, MatchFailure> {
        let mut resumed = event;
        loop {
            let from = match resumed.take() {
                Some(false) => None,
                Some(true) => Some(t.rest),
                None => loop {
                    let cursor = self
                        .cursors
                        .get_mut((t.cur_base + t.level) as usize)
                        .ok_or(MatchFailure::Internal)?;
                    match cursor.next() {
                        Some(c) => {
                            self.budget.charge()?;
                            t.cand = c;
                            break Some(self.compiled.compound(t.chain, t.level as usize)?.start);
                        }
                        None if t.level == 0 => {
                            return Ok(Outcome::Done(false));
                        }
                        None => {
                            // Level `level` searched the whole subtree of the
                            // candidate below it (a descendant combinator) and
                            // the rest of the chain matched nowhere in it. That
                            // depends only on where `level` stands, so no
                            // candidate inside that subtree can do better: the
                            // level below skips it. Without this, `:has(* x)`
                            // over a chain n deep walked n^2 nodes per subject,
                            // and `:has(* * x)` ran into the work budget.
                            let exhausted = self.cursors.pop();
                            t.level -= 1;
                            if matches!(exhausted, Some(HasCursor::Subtree { .. })) {
                                if let Some(below) =
                                    self.cursors.get_mut((t.cur_base + t.level) as usize)
                                {
                                    below.prune();
                                }
                            }
                        }
                    }
                },
            };
            let Some(from) = from else {
                continue;
            };
            let compound = self.compiled.compound(t.chain, t.level as usize)?;
            match self.check(compound, from, t.cand)? {
                Check::Defer(i) => {
                    t.rest = i + 1;
                    let child = self.deferred_task(i, t.cand)?;
                    return Ok(Outcome::Spawn(child));
                }
                Check::Done(false) => {}
                Check::Done(true) => {
                    if t.level + 1 == t.chain.len {
                        return Ok(Outcome::Done(true));
                    }
                    let next = self.compiled.compound(t.chain, t.level as usize + 1)?;
                    let cursor = HasCursor::start(next.comb, t.cand)?;
                    self.cursors.falloc_push(cursor).or_oom()?;
                    t.level += 1;
                }
            }
        }
    }
}
