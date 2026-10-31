//! `:nth-child(... of S)` ([`NthOfTask`]): `S` tested at the candidate and
//! at its siblings in the counting direction, ranks noted in the walk's
//! `NthOfMemo` (`css_match::nth_of`).

use core::ffi::c_long;

use super::{Alts, Outcome, Query, Task};
use crate::css_match::dom::CssNode;
use crate::css_match::simple::anb_matches;
use crate::css_match::tree::nth_of_sibling;
use crate::css_match::MatchFailure;

/// `of S` (`lxb_selectors_pseudo_class_function`'s `NTH_CHILD`): is
/// `candidate` in `S` (`counting == false`), then how many of its siblings
/// in the counting direction are - each tested at `node` in turn, noted in
/// `NthOfMemo`'s `seen` from `seen_base` on, until the list ends or a
/// sibling whose rank a walking query already knows.
#[derive(Clone, Copy)]
pub(in crate::css_match) struct NthOfTask<N> {
    pub(super) a: c_long,
    pub(super) b: c_long,
    pub(super) from_end: bool,
    pub(super) alts: Alts,
    /// The `of S` simple selector's index: its ranks' memo key.
    pub(super) sel: u32,
    pub(super) candidate: N,
    pub(super) node: N,
    pub(super) seen_base: u32,
    pub(super) counting: bool,
}

impl<'c, 'p, 'd, N: CssNode<'d>> Query<'c, 'p, 'd, N> {
    /// [`NthOfTask`]: `S` at the candidate itself first - it must match (as in
    /// Lexbor) - then at each sibling in the counting direction, up to the
    /// first whose rank is known: each sibling list is tested once per `of
    /// S` in a walking query, not once per candidate.
    pub(super) fn step_nth_of(
        &mut self,
        t: &mut NthOfTask<N>,
        event: Option<bool>,
    ) -> Result<Outcome<N>, MatchFailure> {
        match event {
            None => {
                if let Some((rank, in_s)) = self.positions.nth_of().rank(t.sel, t.candidate) {
                    return Ok(Outcome::Done(in_s && anb_matches(t.a, t.b, rank)));
                }
                if self.positions.nth_of().known_out(t.sel, t.candidate) {
                    return Ok(Outcome::Done(false));
                }
            }
            Some(in_s) => {
                if !t.counting {
                    if !in_s {
                        self.positions.nth_of().note_out(t.sel, t.candidate)?;
                        return Ok(Outcome::Done(false));
                    }
                    t.counting = true;
                } else {
                    self.positions.nth_of().push_seen(t.node, in_s)?;
                }
                // A sibling already known not to be in `S` is noted without
                // testing `S` again.
                loop {
                    let end_rank = match nth_of_sibling(t.node, t.from_end) {
                        Some(s) => {
                            self.budget.charge()?;
                            let known = self.positions.nth_of().rank(t.sel, s);
                            if known.is_none() {
                                t.node = s;
                            }
                            known.map(|(rank, _)| rank)
                        }
                        None => Some(0),
                    };
                    if let Some(end_rank) = end_rank {
                        let pos = self.positions.nth_of().finish(
                            t.sel,
                            t.seen_base,
                            t.candidate,
                            end_rank,
                        )?;
                        return Ok(Outcome::Done(anb_matches(t.a, t.b, pos)));
                    }
                    if !self.positions.nth_of().known_out(t.sel, t.node) {
                        break;
                    }
                    self.positions.nth_of().push_seen(t.node, false)?;
                }
            }
        }
        let child = Task::Alternatives {
            alts: t.alts,
            node: t.node,
            negate: false,
        };
        Ok(Outcome::Spawn(child))
    }
}
