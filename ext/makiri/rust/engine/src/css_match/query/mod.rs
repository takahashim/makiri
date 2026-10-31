//! The matching machine: one top-level call's state ([`Query`]) and the
//! heap task stack every nested context runs on - Lexbor's entry walk plus
//! its `lxb_selectors_nested_t` contexts. Nesting depth is stack length,
//! never native recursion (the parent module's doc says why). The loop and
//! the nested lists are here; each kind of task's state and step is a
//! module of its own: [`chain`], [`has`] (`:has()`'s forward search) and
//! [`nth_of`] (`of S`).

use falloc::{OomResult, VecGrow};

use super::compile::{Chain, Compiled, Compound, Nest, Step};
use super::dom::CssNode;
use super::plan::QueryPlan;
use super::positions::Positions;
use super::scratch::{recycle, Scratch, SCRATCH_KEEP};
use super::simple::{check_simple, SimpleCheck, SimpleEnv};
use super::{Budget, MatchFailure, QueryShape};

mod chain;
mod has;
mod nth_of;

use chain::ChainTask;
use has::ForwardTask;
pub(super) use has::HasCursor;
use nth_of::NthOfTask;

/// A range of [`Compiled::alts`]: one nested list's alternatives.
#[derive(Clone, Copy)]
pub(super) struct Alts {
    next: u32,
    end: u32,
}

/// One pending piece of a match on [`Query`]'s task stack - Lexbor's entry
/// walk plus its `lxb_selectors_nested_t` contexts, each of which records
/// where to go back to (`return_state`). The task on top runs; one that
/// needs a nested answer pushes the task computing it and waits; a finished
/// one pops and hands its `bool` to the one below. Nesting depth is stack
/// length, never native recursion (module doc).
#[derive(Clone, Copy)]
pub(super) enum Task<N> {
    Chain(ChainTask<N>),
    /// Does `node` match one of `alts` (`:is()`/`:where()`, and `S` of `of
    /// S`), or - `negate`, `:not()` - none of them?
    Alternatives {
        alts: Alts,
        node: N,
        negate: bool,
    },
    /// `:has()`: does one of `alts`' forward searches from `anchor` succeed?
    Has {
        alts: Alts,
        anchor: N,
    },
    Forward(ForwardTask<N>),
    NthOf(NthOfTask<N>),
}

/// What a task step did: finished with a verdict, or wants `Task` answered
/// first (and is then resumed with that answer).
enum Outcome<N> {
    Done(bool),
    Spawn(Task<N>),
}

/// A compound's simple selectors checked from some index: settled, or
/// stopped at the deferred one at this index.
enum Check {
    Done(bool),
    Defer(u32),
}

/// One top-level call's matching state: the compiled selector, the resolved
/// names, the work [`Budget`], and the task stack with the two side stacks
/// its tasks keep their positions in - all kept (cleared, capacity retained)
/// from one candidate to the next, so a walk over many candidates allocates
/// for the deepest match it needed, not once per candidate. Lexbor does the
/// same with its entry / nested-state pools, reset between candidates.
pub(super) struct Query<'c, 'p, 'd, N: CssNode<'d>> {
    compiled: &'c Compiled<'p>,
    /// What the query decided once for its document: names, tag filter.
    plan: QueryPlan<'d, N>,
    budget: Budget,
    tasks: Vec<Task<N>>,
    /// [`ChainTask`]s' compound positions.
    at: Vec<Option<N>>,
    /// [`ForwardTask`]s' per-level candidate cursors.
    cursors: Vec<HasCursor<N>>,
    /// Sibling positions counted so far (walking queries only).
    positions: Positions,
}

impl<'c, 'p, 'd, N: CssNode<'d>> Query<'c, 'p, 'd, N> {
    /// A query of `shape` over `compiled` (a walking one's plan,
    /// [`QueryPlan::new`]).
    pub(super) fn new(
        compiled: &'c Compiled<'p>,
        shape: QueryShape<'d, N>,
        scratch: &mut Scratch<N::Host>,
    ) -> Result<Self, MatchFailure> {
        let QueryShape {
            walk_in,
            scope,
            limit,
        } = shape;
        Ok(Query {
            compiled,
            plan: QueryPlan::new(compiled, walk_in, scratch)?,
            budget: Budget {
                spent: std::cell::Cell::new(0),
                limit,
                scope: scope.map(CssNode::table_key),
            },
            tasks: recycle(core::mem::take(&mut scratch.tasks), usize::MAX),
            at: recycle(core::mem::take(&mut scratch.at), usize::MAX),
            cursors: recycle(core::mem::take(&mut scratch.cursors), usize::MAX),
            positions: Positions::reuse(&mut scratch.positions, walk_in.is_some()),
        })
    }

    /// Hand the names and stacks back to `scratch` for the next query.
    pub(super) fn finish(self, scratch: &mut Scratch<N::Host>) {
        self.plan.give_back(scratch);
        scratch.tasks = recycle(self.tasks, SCRATCH_KEEP);
        scratch.at = recycle(self.at, SCRATCH_KEEP);
        scratch.cursors = recycle(self.cursors, SCRATCH_KEEP);
        self.positions.give_back(&mut scratch.positions);
    }

    /// The compiled selector the query runs.
    pub(super) fn compiled(&self) -> &'c Compiled<'p> {
        self.compiled
    }

    /// Does `node` match any top-level alternative?
    pub(super) fn matches_top(&mut self, node: N) -> Result<bool, MatchFailure> {
        if self.plan.refuses(node) {
            return Ok(false);
        }
        for i in 0..self.compiled.top.len() {
            let chain = self.compiled.top.get(i).ok_or(MatchFailure::Internal)?;
            if chain.len != 0 && self.run(chain, node)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Run the machine from matching `chain` at `node` to its verdict. The
    /// loop is the WHOLE control flow - no Rust-level recursion anywhere here, however deeply
    /// the selector nests.
    fn run(&mut self, chain: Chain, node: N) -> Result<bool, MatchFailure> {
        // The rightmost compound first, with nothing set up: most candidates
        // fail it, and a one-compound chain needs nothing more.
        let idx = chain.len - 1;
        let last = self.compiled.compound(chain, idx as usize)?;
        self.budget.charge()?;
        let (rest, deferred) = match self.check(last, last.start, node)? {
            Check::Done(false) => return Ok(false),
            Check::Done(true) if idx == 0 => return Ok(true),
            Check::Done(true) => (last.end, None),
            Check::Defer(i) => (i + 1, Some(i)),
        };
        let mut first = self.chain_task(chain, node)?;
        first.rest = rest;
        let child = match deferred {
            Some(i) => self.deferred_task(i, node)?,
            // Resumed as matched: `rest` is past the compound's end.
            None => match self.step_chain(&mut first, Some(true))? {
                Outcome::Done(verdict) => {
                    self.at.truncate(first.at_base as usize);
                    return Ok(verdict);
                }
                Outcome::Spawn(child) => child,
            },
        };
        // An earlier run that failed ended the query; this only drops what
        // that left behind.
        self.tasks.clear();
        self.tasks.falloc_push(Task::Chain(first)).or_oom()?;
        self.tasks.falloc_push(child).or_oom()?;
        let mut event = None;
        loop {
            let top = self.tasks.len().wrapping_sub(1);
            let mut task = *self.tasks.get(top).ok_or(MatchFailure::Internal)?;
            match self.step(&mut task, event)? {
                Outcome::Spawn(child) => {
                    if let Some(slot) = self.tasks.get_mut(top) {
                        *slot = task;
                    }
                    self.tasks.falloc_push(child).or_oom()?;
                    event = None;
                }
                Outcome::Done(verdict) => {
                    self.tasks.pop();
                    self.release(task);
                    if self.tasks.is_empty() {
                        return Ok(verdict);
                    }
                    event = Some(verdict);
                }
            }
        }
    }

    /// Give back the side-stack entries a finished `task` held.
    fn release(&mut self, task: Task<N>) {
        match task {
            Task::Chain(t) => self.at.truncate(t.at_base as usize),
            Task::Forward(t) => self.cursors.truncate(t.cur_base as usize),
            Task::NthOf(t) => self.positions.nth_of().truncate_seen(t.seen_base),
            /* No side stack: a new kind that holds one must say so here. */
            Task::Alternatives { .. } | Task::Has { .. } => {}
        }
    }

    /// Advance `task`: from its start (`event == None`) or with the verdict
    /// of the task it spawned.
    fn step(
        &mut self,
        task: &mut Task<N>,
        event: Option<bool>,
    ) -> Result<Outcome<N>, MatchFailure> {
        match task {
            Task::Chain(t) => self.step_chain(t, event),
            Task::Alternatives { alts, node, negate } => {
                if event == Some(true) {
                    // This alternative matched: :is/:where succeeds; :not is
                    // disproved. Either way, the rest need not be tried.
                    return Ok(Outcome::Done(!*negate));
                }
                Ok(match self.next_alternative(alts)? {
                    Some(chain) => Outcome::Spawn(Task::Chain(self.chain_task(chain, *node)?)),
                    // Out of alternatives: :is/:where found none (false);
                    // :not found none that matched, so it holds (true).
                    None => Outcome::Done(*negate),
                })
            }
            Task::Has { alts, anchor } => {
                if event == Some(true) {
                    return Ok(Outcome::Done(true));
                }
                Ok(match self.next_alternative(alts)? {
                    Some(chain) => Outcome::Spawn(self.forward_task(chain, *anchor)?),
                    None => Outcome::Done(false),
                })
            }
            Task::Forward(t) => self.step_forward(t, event),
            Task::NthOf(t) => self.step_nth_of(t, event),
        }
    }

    /// The first non-empty chain left in `alts`, or `None`.
    fn next_alternative(&self, alts: &mut Alts) -> Result<Option<Chain>, MatchFailure> {
        while alts.next < alts.end {
            let chain = self
                .compiled
                .alts
                .get(alts.next as usize)
                .ok_or(MatchFailure::Internal)?;
            alts.next += 1;
            if chain.len != 0 {
                self.budget.charge()?;
                return Ok(Some(chain));
            }
        }
        Ok(None)
    }

    /// The task answering the deferred simple selector `i` at `node`.
    fn deferred_task(&self, i: u32, node: N) -> Result<Task<N>, MatchFailure> {
        let sel = self
            .compiled
            .simples
            .get(i as usize)
            .ok_or(MatchFailure::Internal)?;
        let alts = Alts {
            next: sel.alts,
            end: sel.alts + sel.n_alts,
        };
        Ok(match sel.nest {
            Nest::Has => Task::Has { alts, anchor: node },
            Nest::List { negate } => Task::Alternatives { alts, node, negate },
            Nest::NthOf { a, b, from_end } => Task::NthOf(NthOfTask {
                a,
                b,
                from_end,
                alts,
                sel: i,
                candidate: node,
                node,
                seen_base: self.positions.nth_of_memo().seen_base(),
                counting: false,
            }),
            // Only a nested selector defers: a broken invariant.
            Nest::None => return Err(MatchFailure::Internal),
        })
    }

    /// [`Query::check_compound`], with a [`Step::inline`] list-pseudo
    /// answered in place rather than deferred. Kept out of that function's
    /// loop on purpose: the branch there, never taken by a flat compound,
    /// still cost plain scans ~10% (`.item`, `main a`).
    #[inline]
    fn check(&mut self, compound: Compound, mut from: u32, node: N) -> Result<Check, MatchFailure> {
        loop {
            let i = match self.check_compound(compound, from, node)? {
                Check::Defer(i) => i,
                done => return Ok(done),
            };
            let compiled = self.compiled;
            let sel = compiled
                .simples
                .as_slice()
                .get(i as usize)
                .ok_or(MatchFailure::Internal)?;
            if !sel.inline {
                return Ok(Check::Defer(i));
            }
            if !self.inline_alternatives(sel, node)? {
                return Ok(Check::Done(false));
            }
            from = i + 1;
        }
    }

    /// `compound`'s simple selectors from `from` at `node`, until one fails,
    /// all pass, or one defers.
    #[inline]
    fn check_compound(
        &mut self,
        compound: Compound,
        from: u32,
        node: N,
    ) -> Result<Check, MatchFailure> {
        let compiled = self.compiled;
        let steps = compiled
            .simples
            .as_slice()
            .get(from as usize..compound.end as usize)
            .ok_or(MatchFailure::Internal)?;
        let mut env = SimpleEnv {
            ns: &compiled.ns,
            budget: &self.budget,
            positions: &mut self.positions,
        };
        for (i, sel) in (from..).zip(steps) {
            let name = self.plan.name(i, sel);
            match check_simple(sel, name, node, &mut env)? {
                SimpleCheck::Result(true) => {}
                SimpleCheck::Result(false) => return Ok(Check::Done(false)),
                SimpleCheck::Deferred => return Ok(Check::Defer(i)),
            }
        }
        Ok(Check::Done(true))
    }

    /// A [`Step::inline`] `:is()` / `:where()` / `:not()` at `node`: each
    /// alternative is one flat compound, checked here directly - what an
    /// `Alternatives` task would do, without the task. Every alternative
    /// tried charges the budget, as [`Query::next_alternative`] does.
    #[inline(never)]
    fn inline_alternatives(&mut self, sel: &Step<'_>, node: N) -> Result<bool, MatchFailure> {
        let Nest::List { negate } = sel.nest else {
            return Err(MatchFailure::Internal); /* only a list is inline */
        };
        let compiled = self.compiled;
        for k in sel.alts..sel.alts + sel.n_alts {
            let chain = compiled
                .alts
                .get(k as usize)
                .ok_or(MatchFailure::Internal)?;
            if chain.len == 0 {
                continue;
            }
            self.budget.charge()?;
            // Flat (`mark_inline`), so this never defers - and it keeps
            // `check_compound` the one caller of `check_simple`, which is
            // what lets that be inlined into the hot loop.
            let compound = compiled.compound(chain, 0)?;
            let matched = match self.check_compound(compound, compound.start, node)? {
                Check::Done(m) => m,
                Check::Defer(_) => return Err(MatchFailure::Internal),
            };
            if matched {
                return Ok(!negate);
            }
        }
        Ok(negate)
    }
}
