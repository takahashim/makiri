//! The matching machine: one top-level call's state ([`Query`]) and the
//! heap task stack every nested context runs on - Lexbor's entry walk plus
//! its `lxb_selectors_nested_t` contexts. Nesting depth is stack length,
//! never native recursion (the parent module's doc says why).

use crate::falloc::{OomResult, VecPush};
use crate::lexbor::adapter::html::{HtmlDoc, HtmlNode, TagId};
use crate::lexbor::css_parser::{Combinator, FunctionArg, ListPseudo, Simple};
use core::ffi::c_long;

use super::compile::{Chain, Compiled, Compound, Step};
use super::positions::Positions;
use super::scratch::{recycle, Scratch, SCRATCH_KEEP};
use super::simple::{anb_matches, check_simple, Name, Names, SimpleCheck};
use super::tree::{next_sibling_element, nth_of_sibling, parent_element, prev_sibling_element};
use super::{Budget, MatchFailure};

/// A resumable position in `:has()`'s forward search for ONE compound step -
/// the heap state a [`ForwardTask`] keeps per level so a `:has()` argument's
/// candidate search (potentially many candidates, unlike the main matcher's
/// single-path ancestor/sibling climbs) never recurses natively, whatever the
/// combinator (module doc). Built once per compound step
/// ([`HasCursor::start`]) from the node the search starts FROM, then
/// [`HasCursor::next`] repeatedly for each candidate - same candidates, same
/// order, same element-only filtering as Lexbor's own forward search
/// (§A-4)/this port's earlier native-recursive `has_forward`.
pub(super) enum HasCursor<'doc> {
    /// The combinator can never find anything (`Close` - defensive, per
    /// `Compiled::add_chain`: a compound boundary is always a real
    /// structural combinator, so this is not a real case in practice).
    Empty,
    /// `Combinator::Descendant`: the whole subtree, via
    /// [`HtmlNode::preorder_next`] directly (not [`HtmlNode::subtree`], which
    /// bundles the `.skip(1)` this cursor needs to resume mid-walk instead
    /// of re-deriving on every step).
    Subtree {
        current: HtmlNode<'doc>,
        root: HtmlNode<'doc>,
    },
    /// `Combinator::Child`: the next child to try.
    Child(Option<HtmlNode<'doc>>),
    /// `Combinator::NextSibling`: exactly one candidate, taken then spent.
    NextSibling(Option<HtmlNode<'doc>>),
    /// `Combinator::SubsequentSibling`: the next following sibling to try.
    SubsequentSibling(Option<HtmlNode<'doc>>),
}

impl<'doc> HasCursor<'doc> {
    /// Start searching for `comb`'s candidates from `from`. `Err(Unsupported)`
    /// for the column combinator `||`, exactly as the original `has_forward`
    /// raised it - Lexbor's own traversal cannot run it either.
    fn start(comb: Combinator, from: HtmlNode<'doc>) -> Result<Self, MatchFailure> {
        Ok(match comb {
            Combinator::Descendant => HasCursor::Subtree {
                current: from,
                root: from,
            },
            Combinator::Child => HasCursor::Child(from.first_child()),
            Combinator::NextSibling => HasCursor::NextSibling(next_sibling_element(from)),
            Combinator::SubsequentSibling => {
                HasCursor::SubsequentSibling(next_sibling_element(from))
            }
            Combinator::Close => HasCursor::Empty,
            Combinator::Other => return Err(MatchFailure::Unsupported),
        })
    }

    /// The next ELEMENT candidate, or `None` once the search is exhausted -
    /// [`Query::step_forward`] charges the budget for each one.
    fn next(&mut self) -> Option<HtmlNode<'doc>> {
        match self {
            HasCursor::Empty => None,
            HasCursor::Subtree { current, root } => {
                while let Some(n) = current.preorder_next(*root) {
                    *current = n;
                    if n.element().is_some() {
                        return Some(n);
                    }
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

/// A range of [`Compiled::alts`]: one nested list's alternatives.
#[derive(Clone, Copy)]
pub(super) struct Alts {
    next: u32,
    end: u32,
}

/// Match `chain` right to left - Lexbor's `lxb_selectors_state_find` /
/// `found_check` / `not_found` over its entries: compound `idx` at `cur`,
/// its simple selectors from `rest` on. `at[at_base + i]` is where compound
/// `i` currently stands, which is what a backtrack moves on.
#[derive(Clone, Copy)]
pub(super) struct ChainTask<'doc> {
    chain: Chain,
    idx: u32,
    cur: HtmlNode<'doc>,
    rest: u32,
    at_base: u32,
}

/// One `:has()` alternative's forward search from its anchor (§A-4):
/// compound `level` tested at `cand`, from simple selector `rest`; each
/// level's candidates come from `cursors[cur_base + level]`.
#[derive(Clone, Copy)]
pub(super) struct ForwardTask<'doc> {
    chain: Chain,
    level: u32,
    cand: HtmlNode<'doc>,
    rest: u32,
    cur_base: u32,
}

/// `of S` (§D-1): is `node` in `S` (`counting == false`), then how many of
/// its siblings in the counting direction are - `pos` so far.
#[derive(Clone, Copy)]
pub(super) struct NthOfTask<'doc> {
    a: c_long,
    b: c_long,
    from_end: bool,
    alts: Alts,
    node: HtmlNode<'doc>,
    pos: u64,
    counting: bool,
}

/// One pending piece of a match on [`Query`]'s task stack - Lexbor's entry
/// walk plus its `lxb_selectors_nested_t` contexts, each of which records
/// where to go back to (`return_state`). The task on top runs; one that
/// needs a nested answer pushes the task computing it and waits; a finished
/// one pops and hands its `bool` to the one below. Nesting depth is stack
/// length, never native recursion (module doc).
#[derive(Clone, Copy)]
pub(super) enum Task<'doc> {
    Chain(ChainTask<'doc>),
    /// Does `node` match one of `alts` (`:is()`/`:where()`, and `S` of `of
    /// S`), or - `negate`, `:not()` - none of them?
    Alternatives {
        alts: Alts,
        node: HtmlNode<'doc>,
        negate: bool,
    },
    /// `:has()`: does one of `alts`' forward searches from `anchor` succeed?
    Has {
        alts: Alts,
        anchor: HtmlNode<'doc>,
    },
    Forward(ForwardTask<'doc>),
    NthOf(NthOfTask<'doc>),
}

/// What a task step did: finished with a verdict, or wants `Task` answered
/// first (and is then resumed with that answer).
enum Outcome<'doc> {
    Done(bool),
    Spawn(Task<'doc>),
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
pub(super) struct Query<'c, 'p, 'doc> {
    compiled: &'c Compiled<'p>,
    /// The simple selectors' names, resolved in the query's document.
    names: Names<'doc>,
    budget: Budget,
    tasks: Vec<Task<'doc>>,
    /// [`ChainTask`]s' compound positions.
    at: Vec<Option<HtmlNode<'doc>>>,
    /// [`ForwardTask`]s' per-level candidate cursors.
    cursors: Vec<HasCursor<'doc>>,
    /// Sibling positions counted so far (walking queries only).
    positions: Positions,
    /// The resolved type selector of a lone top-level chain's rightmost
    /// compound: a candidate of that document with another tag is refused
    /// before the machine starts.
    tag_filter: Option<(HtmlDoc<'doc>, Option<TagId>)>,
}

impl<'c, 'p, 'doc> Query<'c, 'p, 'doc> {
    /// A query over `compiled`. With `walk_in`, the query walks that
    /// document: names are resolved lazily ([`Name`]), each the first time a
    /// candidate reaches it, as Lexbor's entries are, and the rightmost type
    /// selector of a lone chain is resolved now, for [`Query::tag_filter`].
    /// Without it (`matches?`, one candidate) names are compared as bytes:
    /// a lookup costs more than the one comparison it would save - measured
    /// even resolved lazily, `ul > li.item` 85 -> 117 ns.
    pub(super) fn new(
        compiled: &'c Compiled<'p>,
        walk_in: Option<HtmlDoc<'doc>>,
        limit: u64,
        scratch: &mut Scratch,
    ) -> Result<Self, MatchFailure> {
        let mut names = Names::reuse(&mut scratch.names);
        if walk_in.is_some() {
            for _ in compiled.simples.as_slice() {
                names.push(Name::Unresolved)?;
            }
        }
        let mut tag_filter = None;
        if let (Some(doc), [chain]) = (walk_in, compiled.top.as_slice()) {
            if chain.len != 0 {
                let last = compiled.compound(*chain, chain.len() - 1)?;
                for i in last.start..last.end {
                    let Some(sel) = compiled.simples.as_slice().get(i as usize) else {
                        continue;
                    };
                    if let Name::Tag(d, id) = Name::resolve(sel, doc) {
                        names.set(i as usize, Name::Tag(d, id))?;
                        tag_filter = Some((d, id));
                        break;
                    }
                }
            }
        }
        Ok(Query {
            compiled,
            names,
            tag_filter,
            budget: Budget {
                spent: std::cell::Cell::new(0),
                limit,
            },
            tasks: recycle(core::mem::take(&mut scratch.tasks), usize::MAX),
            at: recycle(core::mem::take(&mut scratch.at), usize::MAX),
            cursors: recycle(core::mem::take(&mut scratch.cursors), usize::MAX),
            positions: Positions::new(walk_in.is_some()),
        })
    }

    /// Hand the names and stacks back to `scratch` for the next query.
    pub(super) fn finish(self, scratch: &mut Scratch) {
        self.names.give_back(&mut scratch.names);
        scratch.tasks = recycle(self.tasks, SCRATCH_KEEP);
        scratch.at = recycle(self.at, SCRATCH_KEEP);
        scratch.cursors = recycle(self.cursors, SCRATCH_KEEP);
    }

    /// Does `node` match any top-level alternative?
    pub(super) fn matches_top(&mut self, node: HtmlNode<'doc>) -> Result<bool, MatchFailure> {
        // `type_matches`' own verdict, taken before anything else is set up.
        if let Some((doc, id)) = self.tag_filter {
            if node.owner_document() == doc && (id.is_none() || node.tag_id() != id) {
                return Ok(false);
            }
        }
        for i in 0..self.compiled.top.len() {
            let chain = self.compiled.top.get(i).ok_or(MatchFailure::Unsupported)?;
            if chain.len != 0 && self.run(chain, node)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Run the machine from matching `chain` at `node` to its verdict. The
    /// loop is the WHOLE control flow - no Rust-level recursion anywhere here, however deeply
    /// the selector nests.
    fn run(&mut self, chain: Chain, node: HtmlNode<'doc>) -> Result<bool, MatchFailure> {
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
            let mut task = *self.tasks.get(top).ok_or(MatchFailure::Unsupported)?;
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
    fn release(&mut self, task: Task<'doc>) {
        match task {
            Task::Chain(t) => self.at.truncate(t.at_base as usize),
            Task::Forward(t) => self.cursors.truncate(t.cur_base as usize),
            _ => {}
        }
    }

    /// Advance `task`: from its start (`event == None`) or with the verdict
    /// of the task it spawned.
    fn step(
        &mut self,
        task: &mut Task<'doc>,
        event: Option<bool>,
    ) -> Result<Outcome<'doc>, MatchFailure> {
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
                .ok_or(MatchFailure::Unsupported)?;
            alts.next += 1;
            if chain.len != 0 {
                self.budget.charge()?;
                return Ok(Some(chain));
            }
        }
        Ok(None)
    }

    /// A [`ChainTask`] matching `chain` (non-empty) at `node`.
    fn chain_task(
        &mut self,
        chain: Chain,
        node: HtmlNode<'doc>,
    ) -> Result<ChainTask<'doc>, MatchFailure> {
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

    /// A [`ForwardTask`] searching for `chain` (non-empty) from `anchor`.
    fn forward_task(
        &mut self,
        chain: Chain,
        anchor: HtmlNode<'doc>,
    ) -> Result<Task<'doc>, MatchFailure> {
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

    /// The task answering the deferred simple selector `i` at `node`.
    fn deferred_task(&self, i: u32, node: HtmlNode<'doc>) -> Result<Task<'doc>, MatchFailure> {
        let sel = self
            .compiled
            .simples
            .get(i as usize)
            .ok_or(MatchFailure::Unsupported)?;
        let alts = Alts {
            next: sel.alts,
            end: sel.alts + sel.n_alts,
        };
        Ok(match sel.simple {
            Simple::PseudoClassFunction(FunctionArg::Selectors {
                pseudo: ListPseudo::Has,
                ..
            }) => Task::Has { alts, anchor: node },
            Simple::PseudoClassFunction(FunctionArg::Selectors { pseudo, .. }) => {
                Task::Alternatives {
                    alts,
                    node,
                    negate: pseudo == ListPseudo::Not,
                }
            }
            Simple::PseudoClassFunction(FunctionArg::Nth {
                from_end,
                anb: Some(anb),
                ..
            }) => Task::NthOf(NthOfTask {
                a: anb.a,
                b: anb.b,
                from_end,
                alts,
                node,
                pos: 0,
                counting: false,
            }),
            // `check_simple` defers nothing else: a broken invariant.
            _ => return Err(MatchFailure::Unsupported),
        })
    }

    /// [`Query::check_compound`], with a [`Step::inline`] list-pseudo
    /// answered in place rather than deferred. Kept out of that function's
    /// loop on purpose: the branch there, never taken by a flat compound,
    /// still cost plain scans ~10% (`.item`, `main a`).
    #[inline]
    fn check(
        &mut self,
        compound: Compound,
        mut from: u32,
        node: HtmlNode<'doc>,
    ) -> Result<Check, MatchFailure> {
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
                .ok_or(MatchFailure::Unsupported)?;
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
        node: HtmlNode<'doc>,
    ) -> Result<Check, MatchFailure> {
        let compiled = self.compiled;
        let steps = compiled
            .simples
            .as_slice()
            .get(from as usize..compound.end as usize)
            .ok_or(MatchFailure::Unsupported)?;
        for (i, sel) in (from..).zip(steps) {
            let name = self.name(i, sel, node);
            match check_simple(sel, name, node, &self.budget, &mut self.positions)? {
                SimpleCheck::Result(true) => {}
                SimpleCheck::Result(false) => return Ok(Check::Done(false)),
                SimpleCheck::Deferred => return Ok(Check::Defer(i)),
            }
        }
        Ok(Check::Done(true))
    }

    /// Simple selector `i`'s [`Name`], resolved in `node`'s document the
    /// first time; `Name::None` in a query that compares names as bytes
    /// (it keeps no slots - `Query::new`).
    #[inline]
    fn name(&mut self, i: u32, sel: &Step<'_>, node: HtmlNode<'doc>) -> Name<'doc> {
        match self.names.get_mut(i as usize) {
            Some(slot @ Name::Unresolved) => {
                *slot = Name::resolve(sel, node.owner_document());
                *slot
            }
            Some(n) => *n,
            None => Name::None,
        }
    }

    /// A [`Step::inline`] `:is()` / `:where()` / `:not()` at `node`: each
    /// alternative is one flat compound, checked here directly - what an
    /// `Alternatives` task would do, without the task. Every alternative
    /// tried charges the budget, as [`Query::next_alternative`] does.
    #[inline(never)]
    fn inline_alternatives(
        &mut self,
        sel: &Step<'_>,
        node: HtmlNode<'doc>,
    ) -> Result<bool, MatchFailure> {
        let negate = matches!(
            sel.simple,
            Simple::PseudoClassFunction(FunctionArg::Selectors {
                pseudo: ListPseudo::Not,
                ..
            })
        );
        let compiled = self.compiled;
        for k in sel.alts..sel.alts + sel.n_alts {
            let chain = compiled
                .alts
                .get(k as usize)
                .ok_or(MatchFailure::Unsupported)?;
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
                Check::Defer(_) => return Err(MatchFailure::Unsupported),
            };
            if matched {
                return Ok(!negate);
            }
        }
        Ok(negate)
    }

    fn set_at(&mut self, i: u32, node: HtmlNode<'doc>) {
        if let Some(slot) = self.at.get_mut(i as usize) {
            *slot = Some(node);
        }
    }

    /// [`ChainTask`]: a compound that matches moves left along its
    /// combinator; one that fails backtracks to the nearest choice at or
    /// right of it - a `Descendant` or `SubsequentSibling` combinator whose
    /// compound can move on to the next ancestor / preceding sibling. That
    /// is exhaustive, as Lexbor's `not_found` state is. Every compound test
    /// charges the budget.
    ///
    /// `at[at_base + i]` records where compound `i` last matched; it is read
    /// only when a backtrack comes back to `i` from the left, which only a
    /// match of `i` can have led to.
    fn step_chain(
        &mut self,
        t: &mut ChainTask<'doc>,
        event: Option<bool>,
    ) -> Result<Outcome<'doc>, MatchFailure> {
        let compiled = self.compiled;
        let compounds = compiled
            .compounds
            .as_slice()
            .get(t.chain.start as usize..(t.chain.start + t.chain.len) as usize)
            .ok_or(MatchFailure::Unsupported)?;
        let compound = |i: u32| {
            compounds
                .get(i as usize)
                .copied()
                .ok_or(MatchFailure::Unsupported)
        };
        let last = t.chain.len - 1;
        let mut resumed = event;
        loop {
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
                    Combinator::Other => return Err(MatchFailure::Unsupported),
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
            }
            // `not_found`: move the nearest choice at or right of `idx` on.
            loop {
                if t.idx >= last {
                    return Ok(Outcome::Done(false));
                }
                let moved = match compound(t.idx + 1)?.comb {
                    Combinator::Descendant => parent_element(t.cur),
                    Combinator::SubsequentSibling => prev_sibling_element(t.cur),
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
                        .ok_or(MatchFailure::Unsupported)?;
                }
            }
        }
    }

    /// [`ForwardTask`]: take the next candidate at the current level (or,
    /// out of them, back to the level before), test that level's compound on
    /// it, and on a match go one level deeper from it. The same candidates,
    /// in the same order, as Lexbor's forward search (§A-4).
    fn step_forward(
        &mut self,
        t: &mut ForwardTask<'doc>,
        event: Option<bool>,
    ) -> Result<Outcome<'doc>, MatchFailure> {
        let mut resumed = event;
        loop {
            let from = match resumed.take() {
                Some(false) => None,
                Some(true) => Some(t.rest),
                None => loop {
                    let cursor = self
                        .cursors
                        .get_mut((t.cur_base + t.level) as usize)
                        .ok_or(MatchFailure::Unsupported)?;
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
                            self.cursors.pop();
                            t.level -= 1;
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

    /// [`NthOfTask`]: `S` at the candidate itself first - it must match
    /// (§D-1) - then at each sibling in the counting direction.
    fn step_nth_of(
        &mut self,
        t: &mut NthOfTask<'doc>,
        event: Option<bool>,
    ) -> Result<Outcome<'doc>, MatchFailure> {
        match event {
            None => {}
            Some(in_s) => {
                if !t.counting {
                    if !in_s {
                        return Ok(Outcome::Done(false));
                    }
                    t.counting = true;
                    t.pos = 1;
                } else {
                    t.pos += u64::from(in_s);
                }
                match nth_of_sibling(t.node, t.from_end) {
                    Some(s) => {
                        self.budget.charge()?;
                        t.node = s;
                    }
                    None => {
                        return Ok(Outcome::Done(anb_matches(t.a, t.b, t.pos)));
                    }
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
