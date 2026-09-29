//! CSS selector matcher over the typed HTML adapter, structured as an
//! explicit (heap) work stack rather than native recursion - the same
//! architectural property Lexbor's own `lxb_selectors_*` state machine has
//! (`notes/css_selectors_crate_migration_plan.ja.md` §1.1: `do { entry =
//! selectors->state(...) } while (entry != NULL);` never recurses on
//! selector nesting). Selector PARSING is not reimplemented: it goes through
//! the existing `lexbor::css_parser` typed view, the same one the XML
//! CSS->XPath lowering already uses.
//!
//! Wired into `Node#css`/`#at_css`/`#matches?` (`glue::html_node::css`), which
//! is Phase 3 of `notes/css_selectors_crate_migration_plan.ja.md` - see that
//! plan's §1.1 for why this port (its "(B)-as-port") was chosen over adopting
//! the `selectors`/`cssparser` crates ("(A)", explored then discarded; its
//! spike code is gone, findable only through git history).
//!
//! # This is a semantic port of Lexbor's `selectors.c`, not a code port
//!
//! `notes/lexbor_selectors_c_semantics.ja.md` records a systematic reading of
//! `vendor/lexbor/source/lexbor/selectors/selectors.c` (matching logic) and
//! its test suite (`vendor/lexbor/test/lexbor/selectors/selectors.c`); every
//! matching rule below is cross-referenced against it by section letter
//! (`§B-1` etc.).
//!
//! The control flow is Lexbor's too, in safe Rust: a chain is matched right
//! to left by the `find` / `found_check` / `not_found` loop over its compounds
//! (`Query::step_chain`, backtracking to the nearest descendant / subsequent-
//! sibling combinator on a failure), each simple selector is decoded once
//! (`Step`, Lexbor's entry), names are resolved once per query (`Name`,
//! Lexbor's `entry->id`), and `#id` / `.class` read the element's own
//! `attr_id` / `attr_class` shortcut. A nested list-pseudo is a nested context
//! on a heap stack (`Task`, Lexbor's `lxb_selectors_nested_t`) that the loop
//! returns to with its verdict. What is NOT carried over is the C file's
//! intrusive linked lists over a hand-written object pool: the tables are
//! index-addressed (`Compiled`), and the stacks are `falloc` vectors.
//!
//! An earlier version drove everything - every compound of every chain -
//! through a general continuation machine (a frame stack plus a slab of
//! parked continuations). It was correct and recursion-free, and it was the
//! whole gap to Lexbor: 1.7-3.5x Lexbor's time on the probes, against
//! 1.0-1.6x once the loop above took over.
//!
//! Deliberate, documented departures from a byte-for-byte semantic
//! match, plus what is still open:
//!
//! - `:disabled`, `:enabled` and `:checked` follow the HTML Standard's
//!   definitions (`is_disabled`'s doc), not `lxb_selectors_pseudo_class*`:
//!   an `<input>` in a `<fieldset disabled>` is disabled, the legend
//!   exemption is the fieldset's first `legend` element child (Lexbor reads
//!   `first_child`, so whitespace defeats it and an empty fieldset is a NULL
//!   read, §C-2), `option`/`optgroup` count, `:enabled` is only for the
//!   elements `:disabled` is defined for (Lexbor: any element), and an
//!   element with a custom tag is neither `:disabled` nor `:checked` by its
//!   attribute alone. Checked against the Standard
//!   (`form_state_pseudo_classes_follow_the_html_standard`); the Lexbor
//!   differential fuzzer leaves them out.
//! - `lxb_selectors_anb_calc` (§D-3) tests `:nth-*`'s `An+B` with a `double`
//!   division, which past 2^53 answers "divisible" for everything; this port
//!   computes exactly, in `i128` (`anb_matches`'s doc).
//! - `:nth-child(An+B of S)` / `:nth-last-child(An+B of S)` (§D-1) count by
//!   the CSS definition - an element in `S`, ranked among its element
//!   siblings that are in `S` - where Lexbor miscounts in many shapes: a
//!   comma list in `S` (it starts from the LAST list, `anb->of->last`, so
//!   `p:nth-child(2 of ul, p)` and `(2 of p, ul)` answer differently), a
//!   combinator in `S` (`span:nth-child(2 of li span)` finds a `<span>` that
//!   is first among `li span` siblings), and even simple pseudos such as
//!   `:enabled` or `:empty` in `S`, or the ORDER of simple selectors in its
//!   compound (`[data-n^='1']:nth-child(odd)` vs `:nth-child(odd)[data-n^='1']`).
//!   This port's answer is checked against a spec oracle rather than Lexbor
//!   (`lexbor::tests::selector_port_spike::nth_child_of_s_agrees_with_a_spec_oracle`);
//!   the Lexbor differential fuzzer leaves `of S` out. The walk itself is
//!   Lexbor's shape - candidate in `S`, then siblings one at a time
//!   (`NthOfTask`) on the task stack.
//! - An attribute selector's NAME is looked up as the DOM's `getAttribute`
//!   does - ASCII case-insensitive on an HTML element in an HTML document,
//!   case-sensitive otherwise, the HTML Standard's rule for Selectors - where
//!   Lexbor folds case everywhere (so `[viewbox]` finds an SVG `viewBox`
//!   there, not here). Resolving names to Lexbor's ids ([`Name`]) keeps this
//!   rule exactly (an id match is only a pre-filter);
//!   `lexbor::tests::selector_port_spike::resolved_names_agree_with_the_old_engine`
//!   pins that this is the ONLY name-lookup difference.
//! - `#id` / `.class` read the DOM's ID and class attributes - the
//!   no-namespace `id` / `class` - through Lexbor's shortcut, as Lexbor
//!   does; a lookup by qualified name would also take an unprefixed `id` set
//!   IN a namespace (`setAttributeNS("urn:x", "id")`), which is not the ID.
//!
//! Still open (tracked in the plan, not silent gaps): `::pseudo-elements`,
//! `:lexbor-contains()` (decided not to reimplement), `:current()` (deferred,
//! not ruled out - `notes/css_selectors_crate_migration_plan.ja.md`). Closed,
//! not open: the selector-nesting cap (see the next section), the work budget
//! ([`Budget`]), and allocation - every table and stack a query
//! uses is query-local and grows through `falloc` ([`Compiled`], `Query`), so
//! an out-of-memory raises [`MatchFailure::Oom`] instead of aborting, and
//! `rake oom`'s `css` scenario fails each site in turn.
//!
//! # Why an explicit stack, not "just write it recursively"
//!
//! Matching `:is(:is(:is(...)))` the natural, direct way - a function that
//! checks a compound and, on hitting a nested list-pseudo, calls itself
//! again for the nested list - reproduces exactly the `selectors`/
//! `cssparser` crate's problem (measured stack-overflowing at a nesting
//! depth of only ~2,000-2,500 in a release build). The task stack below
//! avoids that: nesting depth becomes heap growth, never call-stack growth -
//! verified at 500,000 levels in `lexbor::tests::selector_port_spike`.
//!
//! `:has()`'s own forward search (§A-4) is heap-based too, for the same
//! reason and the same way: `ForwardTask`/`HasCursor` walk candidates and
//! `:has()`-inside-`:has()` nesting without ever making a native Rust call
//! that itself recurses. This was NOT the original design - an earlier
//! version answered `:has()` with ordinary Rust recursion (`has_forward`),
//! reasoning that `MAX_COMPOUNDS` (64, a fixed complexity cap already
//! enforced when a chain is compiled) bounded it safely. That bound was real
//! for ONE `:has()` chain's own compound-by-compound depth, but said nothing
//! about `:has()` NESTED inside another `:has()`'s argument: `check_simple`'s
//! `Has` arm called `has_matches` EAGERLY (unlike `:is`/`:where`/`:not`,
//! which already deferred to a heap stack), so each nesting level
//! added a fresh native call chain with its OWN 64-deep budget, unbounded by
//! nesting depth - confirmed as a real, reachable crash: `:has(` × 300 `div`
//! `)` × 300 against a 302-deep document, inside a `Fiber` given only
//! `RUBY_FIBER_MACHINE_STACK_SIZE`'s documented minimum (128 KiB), crashed
//! the WHOLE PROCESS with an uncaught `SystemStackError` that escaped every
//! `rescue`. Lexbor's own C engine never had this problem: `:has()`/`:is()`
//! nesting is handled by `lxb_selectors_nested_t`, a heap structure, not C
//! recursion (`notes/css_selectors_crate_migration_plan.ja.md` §1.1) - the
//! fix here is bringing `:has()` in line with what Lexbor (and this file's
//! own `:is()`/`:where()`/`:not()`) already do, not inventing a new
//! technique. `MAX_COMPOUNDS` still bounds one `:has()` chain's length (a
//! sanity cap, not a stack-safety one - see its doc), but nesting depth is
//! now unbounded the same way `:is()` nesting is, verified the same way (a
//! stress test at a depth far beyond the crash threshold, on a
//! `RUBY_FIBER_MACHINE_STACK_SIZE`-sized thread stack, in
//! `lexbor::tests::selector_port_spike`).

#![forbid(unsafe_code)]

use crate::falloc::{OomResult, VecPush};
use crate::lexbor::adapter::html::{
    AttrName, HtmlAttr, HtmlDoc, HtmlElement, HtmlNode, NodeType, NsId, RawNode, TagId,
};
use crate::lexbor::css_parser::{
    AttrMatch, Combinator, FunctionArg, List, ListPseudo, Lists, PseudoClass, Simple,
};
use crate::limits::NODE_SET_MAX;
use crate::ptr_table::PtrMap;
use core::ffi::c_long;

/// A complexity bound on compounds per chain, mirroring `css::MAX_COMPOUNDS`.
///
/// Not a stack-safety mechanism (this design needs none, for `:has()` nesting
/// or any other - see the module doc) - just a sanity cap so a single
/// compound chain (either the main match or one `:has()` alternative) can't
/// grow unboundedly.
pub(crate) const MAX_COMPOUNDS: usize = 64;

/// The per-query work budget's default cap - the count [`Budget::charge`]
/// compares against. On the same scale as [`NODE_SET_MAX`]: this bounds not
/// the RESULT set but the total number of steps one top-level call
/// ([`matches_any`], [`select_all`], [`select_first`]) may take
/// charging it, which is what stops a `:has()` search from multiplying its
/// cost per candidate element into something unbounded by the document's own
/// size.
const DEFAULT_WORK_BUDGET: u64 = 10 * 1000 * 1000;

/// One top-level call's work budget: every step that can cost MORE than the
/// input document/selector's own size bounds already (concretely: each
/// compound tested, each `:has()` candidate visited, each alternative tried
/// and each sibling `:nth-*` counts) charges it once. Exceeding it is
/// [`MatchFailure::WorkExceeded`] - a hard stop propagated all the way back
/// to the caller, never a silent `false` for just the one `:has()` that
/// happened to hit it: a `:has()` inside a larger compound answering `false`
/// because ITS OWN search ran out of budget would be a wrong verdict for that
/// compound, not merely an incomplete one, and CLAUDE.md's fail-closed rule
/// is "raise instead" of that.
///
/// A plain `Cell`, not `xpath::limits::Budget`'s `Rc<RefCell<Error>>` sink:
/// the Ruby glue (`glue::html_node::css`) only needs to know WHICH failure
/// happened, not a formatted diagnostic, so one `u64` counter local to a
/// single call needs no shared ownership.
struct Budget {
    spent: std::cell::Cell<u64>,
    limit: u64,
}

/// Why a match/query could not be answered - propagated as a hard failure,
/// never folded into a silent `false`/empty result (CLAUDE.md's fail-closed
/// rule: a wrong answer is worse than a raise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchFailure {
    /// [`Budget`] ran out: some step - most likely `:has()`'s own search -
    /// charged past its call's cap.
    WorkExceeded,
    /// A construct this port cannot evaluate at all - not "always false" the
    /// way an unimplemented-in-Lexbor-too functional pseudo-class is
    /// (`FunctionArg::Other`, `check_simple`'s doc), but one Lexbor itself
    /// either cannot run (the column combinator `||`, `Combinator::Other` -
    /// Lexbor's own traversal reports an error status for it too) or
    /// implements and this port deliberately does not
    /// (`:lexbor-contains()`, `FunctionArg::Contains` - decided in
    /// `notes/css_selectors_crate_migration_plan.ja.md` §1.1). Answering
    /// `false` for either would be indistinguishable from "genuinely no
    /// element satisfies this", which it is not.
    Unsupported,
    /// A chain somewhere in the selector has more than [`MAX_COMPOUNDS`]
    /// compounds - see [`compile`]'s doc for why this is caught up front
    /// rather than left to a silent never-matching chain.
    TooComplex,
    /// An allocation the match needed failed (`falloc`). Raised, like every
    /// other failure here, never answered as a partial result.
    Oom,
}

impl Budget {
    /// Charge one step. `Err` once the limit is reached - the caller must
    /// stop and propagate it, not answer as if this step had simply failed
    /// to match.
    #[inline]
    fn charge(&self) -> Result<(), MatchFailure> {
        let n = self.spent.get() + 1;
        self.spent.set(n);
        if n > self.limit {
            return Err(MatchFailure::WorkExceeded);
        }
        Ok(())
    }
}

impl crate::falloc::Oom for MatchFailure {
    fn oom() -> Self {
        MatchFailure::Oom
    }
}

/// One of a query's tables: a `falloc` vector, borrowed from a [`Scratch`]
/// for the query and handed back after it, so a warm call reuses the last
/// one's allocation instead of making its own - Lexbor's pools, reset
/// between calls, do the same. (It used to keep its first few items inline,
/// which spared the allocation only for small selectors and cost every
/// call the initialisation and the moves of those inline arrays.)
struct Table<T>(Vec<T>);

impl<T: Copy> Table<T> {
    /// A table over `v`'s allocation (its items are dropped).
    fn reuse<U>(v: &mut Vec<U>) -> Self {
        Table(recycle(core::mem::take(v), usize::MAX))
    }

    /// Give the allocation back to `v`, unless it grew past
    /// [`SCRATCH_KEEP`].
    fn give_back<U>(self, v: &mut Vec<U>) {
        *v = recycle(self.0, SCRATCH_KEEP);
    }

    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn push(&mut self, v: T) -> Result<(), MatchFailure> {
        self.0.falloc_push(v).or_oom()
    }

    #[inline]
    fn as_slice(&self) -> &[T] {
        &self.0
    }

    #[inline]
    fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.0.get_mut(i)
    }

    /// Replace item `i` (< `len`).
    fn set(&mut self, i: usize, v: T) -> Result<(), MatchFailure> {
        *self.get_mut(i).ok_or(MatchFailure::Unsupported)? = v;
        Ok(())
    }

    #[inline]
    fn get(&self, i: usize) -> Option<T> {
        self.0.get(i).copied()
    }
}

/// The tables and stacks a query leaves behind for the next one - Lexbor's
/// `lxb_selectors_t` keeps its entry and nested-state pools across calls the
/// same way, so a warm call allocates nothing. The Ruby glue keeps one in
/// the process-global selector engine (`selector_cache`), under the GVL; the
/// plain entry points ([`select_all`] etc.) start from an empty one.
///
/// Empty between queries: only the capacity carries over, moved between
/// lifetimes by [`recycle`].
pub struct Scratch {
    simples: Vec<Step<'static>>,
    compounds: Vec<Compound>,
    top: Vec<Chain>,
    alts: Vec<Chain>,
    pending: Vec<(Lists<'static>, u32)>,
    names: Vec<Name<'static>>,
    tasks: Vec<Task<'static>>,
    at: Vec<Option<HtmlNode<'static>>>,
    cursors: Vec<HasCursor<'static>>,
}

/// A [`Scratch`] vector over this many items is dropped rather than kept, so
/// one pathological query does not pin its memory for the process's life.
const SCRATCH_KEEP: usize = 1024;

impl Scratch {
    pub const fn new() -> Self {
        Scratch {
            simples: Vec::new(),
            compounds: Vec::new(),
            top: Vec::new(),
            alts: Vec::new(),
            pending: Vec::new(),
            names: Vec::new(),
            tasks: Vec::new(),
            at: Vec::new(),
            cursors: Vec::new(),
        }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch::new()
    }
}

/// `v`, emptied, as a vector of `U` - or an empty one if its capacity is
/// over `keep`. `T` and `U` differ only in a lifetime here, so the collect
/// reuses `v`'s allocation (std's in-place iteration); were it not to, it
/// would make an empty vector - it never allocates.
fn recycle<T, U>(mut v: Vec<T>, keep: usize) -> Vec<U> {
    if v.capacity() > keep {
        return Vec::new();
    }
    v.clear();
    v.into_iter().filter_map(|_| None).collect()
}

/* ------------------------------------------------------------------ *
 * compiling a selector: every chain, once, into query-local tables    *
 * ------------------------------------------------------------------ */

/// A compound (a `Close`-linked run of simple selectors, `simples[start ..
/// end]` of the [`Compiled`] table, in written order) plus the combinator
/// that attaches it to the compound BEFORE it (to its left - more
/// ancestor/earlier-sibling-ward) in the written selector. The same index
/// keeps a [`Query`]'s resolved names.
#[derive(Clone, Copy)]
struct Compound {
    start: u32,
    end: u32,
    comb: Combinator,
}

/// A compound chain: `compounds[start .. start + len]` of the query's
/// [`Compiled`] table, left to right as written. `len == 0` is an empty
/// chain, which never matches.
///
/// `Copy`: two integers into the table, which a task carries as it is.
#[derive(Clone, Copy, Default)]
struct Chain {
    start: u32,
    len: u32,
}

impl Chain {
    fn len(self) -> usize {
        self.len as usize
    }
}

/// One simple selector, decoded once at compile time - Lexbor's own
/// `entry->selector`, read through its `type` switch per candidate, is what
/// this saves re-decoding on every node.
#[derive(Clone, Copy)]
struct Step<'p> {
    simple: Simple<'p>,
    name: &'p [u8],
    /// A list-pseudo's or `of S`'s alternatives: `alts[alts ..
    /// alts + n_alts]` of the [`Compiled`] table.
    alts: u32,
    n_alts: u32,
    /// An attribute selector's value comparison: `Some(ci)` settled at
    /// compile time (an `i` / `s` modifier, or a name outside the §B-5
    /// table), `None` for a table name, case-insensitive on an HTML element
    /// only - Lexbor's per-id `switch`, decided once rather than per node.
    value_ci: Option<bool>,
    /// An `:is()` / `:where()` / `:not()` whose every alternative is one
    /// compound with nothing nested in it: answered in place by
    /// [`Query::check_compound`], with no task.
    inline: bool,
}

/// A selector, compiled for matching: every chain in it - the top-level
/// comma alternatives and every list nested in `:is()`/`:where()`/`:not()`/
/// `:has()`/`of S` - split into compounds ONCE, before any node is tested.
/// Matching then looks a nested list's chain up by the list's identity
/// instead of re-walking the parsed selector for every candidate (which it
/// used to do, allocating a fresh chain each time).
///
/// [`compile`] is also where a selector this matcher will not run is refused
/// (its doc), so the entry points built on it never start matching one.
/// Every table grows through `falloc`: an out-of-memory is
/// [`MatchFailure::Oom`], not an abort.
pub struct Compiled<'p> {
    /// Every simple selector, compound by compound, in the order the
    /// compounds number them.
    simples: Table<Step<'p>>,
    compounds: Table<Compound>,
    /// The top-level comma alternatives, in order.
    top: Table<Chain>,
    /// Every nested list's alternatives, each list's in order ([`Step::alts`]).
    alts: Table<Chain>,
}

/// Compile `groups` - see [`Compiled`] - or refuse it: no chain anywhere
/// over [`MAX_COMPOUNDS`], no `:lexbor-contains()` or column combinator
/// (`||`) anywhere, checked over EVERY comma alternative and EVERY nested
/// list before any node is tested.
///
/// Two bugs the up-front check closes, both about a check that used to
/// happen only LAZILY, mid-match, and so gave an answer that depended on
/// things it never should have:
///
/// - **A too-complex chain used to be silently treated as ABSENT.** The old
///   `collect_compounds` returned `None` both for an EMPTY chain and for one
///   over [`MAX_COMPOUNDS`], and every caller read that as "this alternative
///   cannot match" - correct for empty, wrong for over-limit: an alternative
///   `:not()`/`:is()`/`:has()` drops as if it were never written is invisible
///   to the OR/AND-negated logic around it, so `:not(` a 65-compound chain
///   `)` answered `true` for EVERY element.
/// - **`:lexbor-contains()`/`||` used to be found only if matching reached
///   them.** A type mismatch earlier in the SAME compound, an earlier comma
///   alternative that already answered, or `at_css`'s first-match stop could
///   each skip the construct: `nosuch:lexbor-contains("x")` answered empty
///   while `p:lexbor-contains("x")` raised, and `p, nosuch:lexbor-contains("x")`
///   answered the `<p>`s. The exception must not depend on document content,
///   comma-alternative order, or `at_css`'s early termination.
///
/// Iterative: nested lists wait on an explicit work list, so a selector
/// nested 500,000 deep costs heap, not native stack. A recursive first
/// version turned `:is()` nested 2000 deep into a `SystemStackError` in a
/// 128 KiB `Fiber`, which wedged the shared CSS engine for the process.
pub fn compile(groups: Lists<'_>) -> Result<Compiled<'_>, MatchFailure> {
    compile_in(&mut Scratch::new(), groups)
}

/// [`compile`], into `scratch`'s tables ([`Compiled::give_back`] returns
/// them).
pub fn compile_in<'p>(
    scratch: &mut Scratch,
    groups: Lists<'p>,
) -> Result<Compiled<'p>, MatchFailure> {
    let mut c = Compiled {
        simples: Table::reuse(&mut scratch.simples),
        compounds: Table::reuse(&mut scratch.compounds),
        top: Table::reuse(&mut scratch.top),
        alts: Table::reuse(&mut scratch.alts),
    };
    let mut pending: Table<(Lists<'p>, u32)> = Table::reuse(&mut scratch.pending);
    let built = (|| {
        for list in groups {
            let chain = c.add_chain(list, &mut pending)?;
            c.top.push(chain)?;
        }
        while let Some((lists, at)) = pending.0.pop() {
            for (k, list) in (at..).zip(lists) {
                let chain = c.add_chain(list, &mut pending)?;
                c.alts.set(k as usize, chain)?;
            }
        }
        c.mark_inline()
    })();
    pending.give_back(&mut scratch.pending);
    match built {
        Ok(()) => Ok(c),
        Err(e) => {
            c.give_back(scratch);
            Err(e)
        }
    }
}

/// [`compile`]'s verdict alone, over `scratch`'s tables - for `matches?` on
/// a node no selector can match, which still refuses a selector the matcher
/// would refuse.
pub fn check_compiles(scratch: &mut Scratch, groups: Lists<'_>) -> Result<(), MatchFailure> {
    compile_in(scratch, groups).map(|c| c.give_back(scratch))
}

/// [`compile`]'s verdict alone, for tests that check it without matching.
#[cfg(test)]
pub(crate) fn validate(groups: Lists<'_>) -> Result<(), MatchFailure> {
    compile(groups).map(drop)
}

impl<'p> Compiled<'p> {
    /// Split `list`'s chain into compounds, left to right (the order
    /// `css_parser` links them in), appending them to `compounds`; any list
    /// nested in its simple selectors is queued on `pending`, not walked
    /// here.
    fn add_chain(
        &mut self,
        list: List<'p>,
        pending: &mut Table<(Lists<'p>, u32)>,
    ) -> Result<Chain, MatchFailure> {
        let start = u32::try_from(self.compounds.len()).map_err(|_| MatchFailure::TooComplex)?;
        let mut len = 0u32;
        let mut cur = list.first();
        while let Some(first) = cur {
            len += 1;
            if len as usize > MAX_COMPOUNDS {
                return Err(MatchFailure::TooComplex);
            }
            let comb = first.combinator();
            if comb == Combinator::Other {
                return Err(MatchFailure::Unsupported);
            }
            let simple_start = self.simple_index()?;
            let mut sel = first;
            loop {
                let simple = sel.simple();
                let nested = match simple {
                    Simple::PseudoClassFunction(FunctionArg::Contains(_)) => {
                        return Err(MatchFailure::Unsupported)
                    }
                    Simple::PseudoClassFunction(FunctionArg::Selectors { lists, .. }) => {
                        Some(lists)
                    }
                    Simple::PseudoClassFunction(FunctionArg::Nth { anb, .. }) => {
                        anb.and_then(|a| a.of_list)
                    }
                    _ => None,
                };
                let alts = u32::try_from(self.alts.len()).map_err(|_| MatchFailure::TooComplex)?;
                let mut n_alts = 0u32;
                if let Some(lists) = nested {
                    // Slots now, filled when `pending` reaches the list.
                    for _ in lists {
                        self.alts.push(Chain::default())?;
                        n_alts += 1;
                    }
                    pending.push((lists, alts))?;
                }
                let value_ci = match simple {
                    Simple::Attribute(at) if at.case_insensitive => Some(true),
                    Simple::Attribute(at) if at.explicit_sensitive => Some(false),
                    Simple::Attribute(at)
                        if at.value.is_some() && is_html_ci_attribute(sel.name()) =>
                    {
                        None
                    }
                    _ => Some(false),
                };
                self.simples.push(Step {
                    simple,
                    name: sel.name(),
                    alts,
                    n_alts,
                    value_ci,
                    inline: false,
                })?;
                match sel.next().filter(|n| n.combinator() == Combinator::Close) {
                    Some(n) => sel = n,
                    None => break,
                }
            }
            let compound = Compound {
                start: simple_start,
                end: self.simple_index()?,
                comb,
            };
            self.compounds.push(compound)?;
            cur = sel.next();
        }
        Ok(Chain { start, len })
    }

    /// Hand the tables back to `scratch` for the next query.
    fn give_back(self, scratch: &mut Scratch) {
        self.simples.give_back(&mut scratch.simples);
        self.compounds.give_back(&mut scratch.compounds);
        self.top.give_back(&mut scratch.top);
        self.alts.give_back(&mut scratch.alts);
    }

    /// Set [`Step::inline`] where it holds, once every chain exists.
    fn mark_inline(&mut self) -> Result<(), MatchFailure> {
        for s in 0..self.simples.len() {
            let Some(sel) = self.simples.get(s) else {
                continue;
            };
            let Simple::PseudoClassFunction(FunctionArg::Selectors { pseudo, .. }) = sel.simple
            else {
                continue;
            };
            if pseudo == ListPseudo::Has {
                continue;
            }
            let mut inline = true;
            for k in sel.alts..sel.alts + sel.n_alts {
                let chain = self.alts.get(k as usize).ok_or(MatchFailure::Unsupported)?;
                inline &= match chain.len {
                    0 => true,
                    1 => self.compound_is_flat(self.compound(chain, 0)?)?,
                    _ => false,
                };
            }
            if inline {
                self.simples.set(s, Step { inline, ..sel })?;
            }
        }
        Ok(())
    }

    /// Nothing in `compound` defers ([`check_simple`]'s `Deferred`).
    fn compound_is_flat(&self, compound: Compound) -> Result<bool, MatchFailure> {
        let steps = self
            .simples
            .as_slice()
            .get(compound.start as usize..compound.end as usize)
            .ok_or(MatchFailure::Unsupported)?;
        Ok(steps.iter().all(|s| match s.simple {
            Simple::PseudoClassFunction(FunctionArg::Selectors { .. }) => false,
            Simple::PseudoClassFunction(FunctionArg::Nth { anb, .. }) => {
                anb.and_then(|a| a.of_list).is_none()
            }
            _ => true,
        }))
    }

    fn simple_index(&self) -> Result<u32, MatchFailure> {
        u32::try_from(self.simples.len()).map_err(|_| MatchFailure::TooComplex)
    }

    /// `chain[idx]`. Out of range is a broken invariant, raised rather than
    /// answered.
    fn compound(&self, chain: Chain, idx: usize) -> Result<Compound, MatchFailure> {
        self.compounds
            .get(chain.start as usize + idx)
            .ok_or(MatchFailure::Unsupported)
    }
}

/* ------------------------------------------------------------------ *
 * quirks mode (§B-2/B-3: class/id case folding)                      *
 * ------------------------------------------------------------------ */

/// Lexbor's `compat_mode` values (`adapter::html::HtmlDoc::compat_mode`'s own
/// doc: "0 no-quirks, 1 quirks, 2 limited-quirks").
fn document_is_quirks(node: HtmlNode<'_>) -> bool {
    node.owner_document().compat_mode() == 1
}

/// Lexbor's own whitespace set for tokenizing an attribute value (`class`,
/// or any `~=` operand) - `lexbor_utils_whitespace`: space, tab, LF, FF, CR.
/// Deliberately NOT `u8::is_ascii_whitespace`, which also matches vertical
/// tab (0x0B) - a real behavioural difference documented in
/// `notes/lexbor_selectors_c_semantics.ja.md` §E-2 point 9.
fn is_lexbor_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0C | b'\r')
}

fn eq_bytes(a: &[u8], b: &[u8], case_insensitive: bool) -> bool {
    if case_insensitive {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// §B-2: Lexbor's `lxb_selectors_match_class` - whitespace-tokenize `target`
/// and look for a token equal to `want`. Also the engine behind `~=`
/// (§B-4's `Include`), which is why this takes a `case_insensitive` flag
/// rather than baking in quirks mode itself - the caller decides which rule
/// supplies it (document quirks mode for a bare class selector, `i`/`s`/the
/// HTML case-insensitive attribute table for `~=`).
fn has_whitespace_token(target: &[u8], want: &[u8], case_insensitive: bool) -> bool {
    if want.is_empty() {
        return false; // an empty class name/token never matches (§B-2)
    }
    // Lexbor's own loop: a token is compared only when its length matches.
    let mut rest = target;
    loop {
        let start = rest.iter().position(|&b| !is_lexbor_whitespace(b));
        let Some(tail) = start.and_then(|s| rest.get(s..)) else {
            return false;
        };
        let len = tail
            .iter()
            .position(|&b| is_lexbor_whitespace(b))
            .unwrap_or(tail.len());
        let (tok, after) = tail.split_at(len);
        if len == want.len() && eq_bytes(tok, want, case_insensitive) {
            return true;
        }
        rest = after;
    }
}

/* ------------------------------------------------------------------ *
 * plain (non-deferred) per-simple-selector checks                    *
 * ------------------------------------------------------------------ */

fn is_html_namespace(node: HtmlNode<'_>) -> bool {
    node.ns_id() == Some(NsId::HTML)
}

/// §B-1: `lxb_selectors_match_element` folds ASCII case UNCONDITIONALLY (tag
/// lookup always searches lower-cased), regardless of namespace or quirks
/// mode - unlike class/id (§B-2/B-3, quirks-only) or attributes (§B-5,
/// HTML-namespace-and-document-type-gated). This is a known Lexbor
/// deviation from the CSS spec for foreign content (documented in
/// `NOKOGIRI_DIFFERENCES.md`'s namespace section already), reproduced here
/// for engine parity, not "fixed".
fn name_eq(node: HtmlNode<'_>, want: &[u8]) -> bool {
    // The stored (lower-cased) local name: the DOM's case-preserved one
    // differs from it only in case, which this comparison folds anyway.
    node.element()
        .is_some_and(|el| el.local_name().eq_ignore_ascii_case(want))
}

fn get_attr<'doc>(node: HtmlNode<'doc>, name: &[u8]) -> Option<&'doc [u8]> {
    node.element().and_then(|el| el.get_attribute(name))
}

fn has_attr(node: HtmlNode<'_>, name: &[u8]) -> bool {
    node.element()
        .is_some_and(|el| el.get_attribute(name).is_some())
}

/// §B-5: the 46 HTML attributes whose VALUE compares ASCII case-insensitively
/// when no `i`/`s` modifier is written, and only on an HTML-namespace element
/// of an HTML document (`attribute_matches`). Asked once per selector, by
/// `compile`.
fn is_html_ci_attribute(name: &[u8]) -> bool {
    // Compared case-insensitively against the selector's own attribute name
    // spelling (an author can write `[TYPE=x]`). Lower-cased into a buffer
    // as long as the longest entry, so the table is one `match` (a switch on
    // length, then a compare), not 46 folded compares.
    let mut lower = [0u8; 14];
    let Some(buf) = lower.get_mut(..name.len()) else {
        return false;
    };
    for (d, s) in buf.iter_mut().zip(name) {
        *d = s.to_ascii_lowercase();
    }
    matches!(
        &*buf,
        b"accept"
            | b"accept-charset"
            | b"align"
            | b"alink"
            | b"axis"
            | b"bgcolor"
            | b"charset"
            | b"checked"
            | b"clear"
            | b"codetype"
            | b"color"
            | b"compact"
            | b"declare"
            | b"defer"
            | b"dir"
            | b"direction"
            | b"disabled"
            | b"enctype"
            | b"face"
            | b"frame"
            | b"hreflang"
            | b"http-equiv"
            | b"lang"
            | b"language"
            | b"link"
            | b"media"
            | b"method"
            | b"multiple"
            | b"nohref"
            | b"noresize"
            | b"noshade"
            | b"nowrap"
            | b"readonly"
            | b"rel"
            | b"rev"
            | b"rules"
            | b"scope"
            | b"scrolling"
            | b"selected"
            | b"shape"
            | b"target"
            | b"text"
            | b"type"
            | b"valign"
            | b"valuetype"
            | b"vlink"
    )
}

/// §B-4: `[name op value]` (or `[name]`, existence, when `at.value` is
/// `None`). `value_ci` is [`Step::value_ci`]: `None` - a §B-5 table name with
/// no `i`/`s` - compares case-insensitively on an HTML-namespace element.
///
/// §B-5 also asks that the owner document be an HTML document, a raw
/// document-type read this `#![forbid(unsafe_code)]` module cannot make; it
/// always holds here, since this matcher only ever runs on `Makiri::HTML`
/// documents (XML's CSS goes through `css::lower`), and the namespace alone
/// tells HTML elements from foreign (SVG/MathML) content within one.
fn attribute_matches(
    node: HtmlNode<'_>,
    value: Option<&[u8]>,
    op: AttrMatch,
    at_value: Option<&[u8]>,
    value_ci: Option<bool>,
) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Some(want) = at_value else {
        return true; // `[name]`: existence only (§B-4)
    };
    let ci = value_ci.unwrap_or_else(|| is_html_namespace(node));
    match op {
        AttrMatch::Equal => eq_bytes(value, want, ci),
        // §B-4's `~=` literally reuses the class-token matcher.
        AttrMatch::Include => has_whitespace_token(value, want, ci),
        AttrMatch::Dash => {
            eq_bytes(value, want, ci)
                || (value.len() > want.len()
                    && eq_bytes(&value[..want.len()], want, ci)
                    && value[want.len()] == b'-')
        }
        AttrMatch::Prefix => {
            !want.is_empty()
                && value.len() >= want.len()
                && eq_bytes(&value[..want.len()], want, ci)
        }
        AttrMatch::Suffix => {
            !want.is_empty()
                && value.len() >= want.len()
                && eq_bytes(&value[value.len() - want.len()..], want, ci)
        }
        AttrMatch::Substring => {
            !want.is_empty()
                && want.len() <= value.len()
                && (0..=value.len() - want.len())
                    .any(|i| eq_bytes(&value[i..i + want.len()], want, ci))
        }
        AttrMatch::Other => false,
    }
}

/// The next/previous sibling that is an ELEMENT - for combinator dispatch
/// (`+`, `~`, and the ancestor/parent climb) and `:nth-of-type`-family
/// checks, where the CSS spec (and Lexbor's own structural matching) only
/// ever considers actual elements. NOT for the plain `:nth-child`/
/// `:first-child`/`:last-child`/`:only-child` family - see
/// `next_position_sibling`/`prev_position_sibling` below for why those need
/// a different filter.
fn next_sibling_element(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.next();
    while let Some(n) = cur {
        if n.element().is_some() {
            return Some(n);
        }
        cur = n.next();
    }
    None
}

fn prev_sibling_element(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.prev();
    while let Some(n) = cur {
        if n.element().is_some() {
            return Some(n);
        }
        cur = n.prev();
    }
    None
}

fn parent_element(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    node.parent().filter(|p| p.element().is_some())
}

/// Does `n` count as "a sibling" for the plain (no `of_type`, no `of S`)
/// `:nth-child`/`:nth-last-child`/`:first-child`/`:last-child`/`:only-child`
/// family? Lexbor's own rule (`SEL.c:2055-2080`'s loop condition, and
/// `lxb_selectors_pseudo_class_first_child`/`last_child` the same way,
/// §C-2/§D-1): everything except Text and Comment - NOT "is an element".
/// A `<!doctype html>` (a `DocumentType` node, `<html>`'s own preceding
/// "sibling" under the Document) or an HTML processing-instruction sibling
/// counts here even though neither is an element - found by
/// `agrees_with_the_old_engine_on_randomly_generated_selectors` generating
/// `*:nth-child(2n+1)` and disagreeing with the old engine on `<html>`
/// itself: this port's `prev_sibling_element`/`next_sibling_element`
/// (element-only, correct for combinators and `of_type`/`of S`, WRONG here)
/// skipped the doctype and undercounted its position by one.
fn counts_toward_child_position(n: HtmlNode<'_>) -> bool {
    !matches!(n.node_type(), NodeType::Text | NodeType::Comment)
}

fn next_position_sibling(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.next();
    while let Some(n) = cur {
        if counts_toward_child_position(n) {
            return Some(n);
        }
        cur = n.next();
    }
    None
}

fn prev_position_sibling(node: HtmlNode<'_>) -> Option<HtmlNode<'_>> {
    let mut cur = node.prev();
    while let Some(n) = cur {
        if counts_toward_child_position(n) {
            return Some(n);
        }
        cur = n.prev();
    }
    None
}

fn name_matches_type(a: HtmlNode<'_>, b: HtmlNode<'_>) -> bool {
    // §C-2 (`first_of_type` etc.): namespace-aware, unlike the type selector
    // itself (§B-1, name-only).
    match (a.element(), b.element()) {
        (Some(ea), Some(eb)) => {
            a.ns_id() == b.ns_id() && ea.dom_local_name() == eb.dom_local_name()
        }
        _ => false,
    }
}

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
struct Positions {
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
    fn new(walking: bool) -> Self {
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
fn sibling_position<'doc>(
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

/// The next sibling ELEMENT in `:nth-*(of S)`'s counting direction - toward
/// the end for `:nth-last-child`, toward the start otherwise (§D-1).
fn nth_of_sibling(node: HtmlNode<'_>, from_end: bool) -> Option<HtmlNode<'_>> {
    if from_end {
        next_sibling_element(node)
    } else {
        prev_sibling_element(node)
    }
}

/// §C-1 `:empty` (`SEL.c:1749-1774`): a child of ANY type OTHER than Comment
/// disqualifies it, not just Element/non-empty-text - a processing-instruction
/// child does too (found by `spec/xml_css_spec.rb`'s HTML/XML agreement
/// check: `<i><?pi x?></i>` was wrongly treated as `:empty`, since neither
/// `element()` nor `char_data()` sees a PI, and it fell through unnoticed).
fn is_empty(node: HtmlNode<'_>) -> bool {
    !node.children().any(|c| c.node_type() != NodeType::Comment)
}

/// §C-1 `:blank` (`lxb_dom_node_is_empty`, `node.c:1700-1737`): as `:empty`,
/// but a Text child only disqualifies it when it holds a non-whitespace byte -
/// still stricter than "ignore text entirely", and a PI (or anything else
/// that is neither Text nor Comment) disqualifies it unconditionally, same
/// bug/fix as `is_empty` above.
fn is_blank(node: HtmlNode<'_>) -> bool {
    !node.children().any(|c| match c.char_data() {
        Some(t) => t.iter().any(|&b| !is_lexbor_whitespace(b)),
        None => c.node_type() != NodeType::Comment,
    })
}

fn is_root(node: HtmlNode<'_>) -> bool {
    node.owner_document().as_node().document_root() == Some(node)
}

/// §C-1 `:any-link` / `:link`, exactly as Lexbor has them: an element whose
/// tag is `a`, `area` or `map` (`:any-link`) / `a`, `area` or `link`
/// (`:link`), in any namespace (Lexbor compares the tag id alone, so an SVG
/// `<a>` counts), with an attribute whose local name is `href`, in any
/// namespace (`lxb_dom_element_attr_by_id`, so `xlink:href` counts).
fn is_any_link(node: HtmlNode<'_>, link_tag: bool) -> bool {
    let Some(el) = node.element() else {
        return false;
    };
    let tags: [&[u8]; 3] = if link_tag {
        [b"a", b"area", b"link"]
    } else {
        [b"a", b"area", b"map"]
    };
    tags.contains(&el.local_name()) && el.attrs().any(|a| a.local_name() == b"href")
}

/// An HTML element named one of `names` (the stored, lower-cased local name).
fn html_named(node: HtmlNode<'_>, names: &[&[u8]]) -> bool {
    is_html_namespace(node)
        && node
            .element()
            .is_some_and(|el| names.contains(&el.local_name()))
}

/// Whether `node` is inside a `<fieldset disabled>` without being inside
/// that fieldset's first `legend` element child - the HTML Standard's
/// inheritance for form controls and fieldsets. The first `legend` CHILD,
/// not the first child: whitespace or another element may come before it.
/// Every disabled fieldset on the way up counts, so a legend exempts only
/// from its own fieldset.
fn in_disabled_fieldset(node: HtmlNode<'_>) -> bool {
    let mut child = node;
    let mut ancestor = parent_element(node);
    while let Some(a) = ancestor {
        if html_named(a, &[b"fieldset"]) && has_attr(a, b"disabled") {
            let first_legend = a.children().find(|c| html_named(*c, &[b"legend"]));
            if first_legend != Some(child) {
                return true;
            }
        }
        child = a;
        ancestor = parent_element(a);
    }
    false
}

/// `:disabled`, as the HTML Standard defines it (§4.16.3 and "disabled" for
/// each element): a `button`/`input`/`select`/`textarea` or `fieldset` with
/// a `disabled` attribute or inside a disabled fieldset
/// ([`in_disabled_fieldset`]), an `optgroup` with the attribute, an `option`
/// with it or in an `optgroup` with it. HTML elements only.
///
/// Deliberately NOT Lexbor's `lxb_selectors_pseudo_class_disabled` (module
/// doc), which
/// needs the attribute on the element itself (so an `<input>` inside a
/// disabled fieldset is enabled), counts any element with a custom tag, and
/// decides the legend exemption from the fieldset's `first_child` - a
/// whitespace text node defeats it, and an empty fieldset is a NULL read.
///
/// Form-associated custom elements are left out: whether a custom element is
/// form-associated is decided by a script's class definition, which a
/// parsed document does not have. As everywhere else here, the content
/// attribute stands for the element's state: the document as parsed.
fn is_disabled(node: HtmlNode<'_>) -> bool {
    if html_named(
        node,
        &[b"button", b"input", b"select", b"textarea", b"fieldset"],
    ) {
        return has_attr(node, b"disabled") || in_disabled_fieldset(node);
    }
    if html_named(node, &[b"optgroup"]) {
        return has_attr(node, b"disabled");
    }
    if html_named(node, &[b"option"]) {
        return has_attr(node, b"disabled")
            || parent_element(node)
                .is_some_and(|p| html_named(p, &[b"optgroup"]) && has_attr(p, b"disabled"));
    }
    false
}

/// `:enabled`: the elements `:disabled` is defined for, when not disabled -
/// not every other element, as Lexbor's unconditional `!disabled` has it.
fn is_enabled(node: HtmlNode<'_>) -> bool {
    html_named(
        node,
        &[
            b"button",
            b"input",
            b"select",
            b"textarea",
            b"fieldset",
            b"optgroup",
            b"option",
        ],
    ) && !is_disabled(node)
}

/// `:checked`, as the HTML Standard defines it: an `input` whose type is
/// Checkbox or Radio and which is checked, or an `option` that is
/// selected - by the `checked` / `selected` attribute, the parsed state.
/// HTML elements only. Lexbor also takes an element with a custom tag and a
/// `checked` attribute; the Standard does not.
fn is_checked(node: HtmlNode<'_>) -> bool {
    if html_named(node, &[b"option"]) {
        return has_attr(node, b"selected");
    }
    if html_named(node, &[b"input"]) {
        let checkable = get_attr(node, b"type").is_some_and(|t| {
            t.eq_ignore_ascii_case(b"checkbox") || t.eq_ignore_ascii_case(b"radio")
        });
        return checkable && has_attr(node, b"checked");
    }
    false
}

/// §C-1 `:optional`/`:required`: `input`/`select`/`textarea` only.
fn is_form_field(node: HtmlNode<'_>) -> bool {
    node.element()
        .is_some_and(|el| matches!(el.dom_local_name(), b"input" | b"select" | b"textarea"))
}

fn is_read_write(node: HtmlNode<'_>) -> bool {
    node.element()
        .is_some_and(|el| matches!(el.dom_local_name(), b"input" | b"textarea"))
        && !has_attr(node, b"readonly")
        && !is_disabled(node)
}

fn plain_pseudo_matches(
    pc: PseudoClass,
    node: HtmlNode<'_>,
    budget: &Budget,
    positions: &mut Positions,
) -> Result<bool, MatchFailure> {
    Ok(match pc {
        // Not `prev_sibling_element`/`next_sibling_element` - see
        // `counts_toward_child_position`'s doc; a preceding/following
        // doctype or processing-instruction sibling disqualifies these, in
        // Lexbor and so here.
        PseudoClass::FirstChild => prev_position_sibling(node).is_none(),
        PseudoClass::LastChild => next_position_sibling(node).is_none(),
        PseudoClass::OnlyChild => {
            prev_position_sibling(node).is_none() && next_position_sibling(node).is_none()
        }
        PseudoClass::Empty => is_empty(node),
        PseudoClass::Root => is_root(node),
        PseudoClass::FirstOfType => sibling_position(node, false, true, budget, positions)? == 1,
        PseudoClass::LastOfType => sibling_position(node, true, true, budget, positions)? == 1,
        PseudoClass::OnlyOfType => {
            sibling_position(node, false, true, budget, positions)? == 1
                && sibling_position(node, true, true, budget, positions)? == 1
        }
        PseudoClass::AnyLink => is_any_link(node, false),
        PseudoClass::Link => is_any_link(node, true),
        PseudoClass::Blank => is_blank(node),
        PseudoClass::Checked => is_checked(node),
        PseudoClass::Disabled => is_disabled(node),
        PseudoClass::Enabled => is_enabled(node),
        PseudoClass::Optional => is_form_field(node) && !has_attr(node, b"required"),
        PseudoClass::Required => is_form_field(node) && has_attr(node, b"required"),
        PseudoClass::ReadOnly => !is_read_write(node),
        PseudoClass::ReadWrite => is_read_write(node),
        PseudoClass::Active => has_attr(node, b"active"),
        PseudoClass::Focus => has_attr(node, b"focus"),
        PseudoClass::Hover => has_attr(node, b"hover"),
        // §C-1 (`SEL.c:1863-1872`): `input`/`textarea` only (not `select`,
        // unlike `:optional`/`:required`), and only whether `placeholder` is
        // PRESENT - Lexbor never looks at its value or whether the field is
        // actually showing it empty, so an `<input placeholder>` with no
        // value written at all still matches, faithfully reproduced here.
        PseudoClass::PlaceholderShown => {
            node.element()
                .is_some_and(|el| matches!(el.dom_local_name(), b"input" | b"textarea"))
                && has_attr(node, b"placeholder")
        }
        PseudoClass::Other => false,
    })
}

fn nth_matches(
    node: HtmlNode<'_>,
    from_end: bool,
    of_type: bool,
    anb: Option<crate::lexbor::css_parser::Nth<'_>>,
    budget: &Budget,
    positions: &mut Positions,
) -> Result<bool, MatchFailure> {
    let Some(anb) = anb else {
        return Ok(false);
    };
    let pos = sibling_position(node, from_end, of_type, budget, positions)?;
    Ok(anb_matches(anb.a, anb.b, pos))
}

/// §D-3 `lxb_selectors_anb_calc`: is `pos` = `a*n + b` for some `n >= 0`?
/// Exact, where Lexbor divides in `double` - past 2^53 every `double` is an
/// integer, so its divisibility test there always passes (module doc).
///
/// In `i128`: `a` and `b` reach `LONG_MAX` in magnitude (Lexbor clamps them
/// there) and `pos` is a `u64`, so `pos - b` overflows 64 bits - and a
/// release build checks overflow, so `:nth-child(n-9223372036854775807)`
/// panicked. Nothing here can overflow 128 bits: `|k| < 2^65`. Treating an
/// overflow as "no match" instead would be a wrong answer, not a safe one -
/// that selector (a = 1) matches every element.
fn anb_matches(a: c_long, b: c_long, pos: u64) -> bool {
    let (a, b, pos) = (i128::from(a), i128::from(b), i128::from(pos));
    if a == 0 {
        return pos == b;
    }
    let k = pos - b;
    k % a == 0 && k / a >= 0
}

/* ------------------------------------------------------------------ *
 * :has() - §A-4: heap-based forward search (module doc)              *
 * ------------------------------------------------------------------ */

/// A resumable position in `:has()`'s forward search for ONE compound step -
/// the heap state a [`ForwardTask`] keeps per level so a `:has()` argument's
/// candidate search (potentially many candidates, unlike the main matcher's
/// single-path ancestor/sibling climbs) never recurses natively, whatever the
/// combinator (module doc). Built once per compound step
/// ([`HasCursor::start`]) from the node the search starts FROM, then
/// [`HasCursor::next`] repeatedly for each candidate - same candidates, same
/// order, same element-only filtering as Lexbor's own forward search
/// (§A-4)/this port's earlier native-recursive `has_forward`.
enum HasCursor<'doc> {
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

/* ------------------------------------------------------------------ *
 * simple-selector dispatch                                            *
 * ------------------------------------------------------------------ */

/// One simple selector's verdict at a node, or `Deferred`: it is a
/// `:is()`/`:where()`/`:not()`/`:has()`/`of S`, which a nested [`Task`]
/// answers.
enum SimpleCheck {
    Result(bool),
    Deferred,
}

#[inline]
fn check_simple(
    sel: &Step<'_>,
    name: Name<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
    positions: &mut Positions,
) -> Result<SimpleCheck, MatchFailure> {
    Ok(match sel.simple {
        // `*` matches an ELEMENT, never a text/comment/doctype/PI node -
        // found by the same randomized differential test as
        // `counts_toward_child_position`: `:has(*)` used `*` against every
        // node a `:has()` Descendant search visits, and an unconditional
        // `true` here made a `<p>` with only text content wrongly "have"
        // that text node as a `*`-matching descendant.
        Simple::Universal => SimpleCheck::Result(node.element().is_some()),
        Simple::Type => SimpleCheck::Result(type_matches(node, sel.name, name)),
        // §B-3/§B-2 through the element's own `id` / `class` shortcut, as
        // Lexbor reads them (`HtmlElement::id_attr`) - no attribute-list scan.
        Simple::Id => SimpleCheck::Result(
            node.element()
                .and_then(HtmlElement::id_attr)
                .is_some_and(|a| eq_bytes(a.value(), sel.name, document_is_quirks(node))),
        ),
        Simple::Class => SimpleCheck::Result(
            node.element()
                .and_then(HtmlElement::class_attr)
                .is_some_and(|a| {
                    has_whitespace_token(a.value(), sel.name, document_is_quirks(node))
                }),
        ),
        // §B-4: explicit `i` -> case-insensitive; explicit `s` -> forced
        // case-sensitive; no modifier -> the HTML table decides. `compile`
        // settled which in `Step::value_ci`.
        Simple::Attribute(at) => SimpleCheck::Result(attribute_matches(
            node,
            attr_value(node, sel.name, name),
            at.op,
            at.value,
            sel.value_ci,
        )),
        Simple::PseudoClass(pc) => {
            SimpleCheck::Result(plain_pseudo_matches(pc, node, budget, positions)?)
        }
        Simple::PseudoClassFunction(FunctionArg::Nth {
            from_end,
            of_type,
            anb,
        }) => match anb {
            Some(a) if a.of_list.is_some() => SimpleCheck::Deferred,
            _ => SimpleCheck::Result(nth_matches(
                node, from_end, of_type, anb, budget, positions,
            )?),
        },
        Simple::PseudoClassFunction(FunctionArg::Selectors { .. }) => SimpleCheck::Deferred,
        // `:lexbor-contains()`: Lexbor itself matches with it (§D-5) - this
        // port deliberately does not (`MatchFailure::Unsupported`'s doc) -
        // so answering `false` would be indistinguishable from a selector
        // that legitimately matches nothing. Raised instead.
        Simple::PseudoClassFunction(FunctionArg::Contains(_)) => {
            return Err(MatchFailure::Unsupported)
        }
        // Any OTHER functional pseudo-class (`:dir()`, `:lang()`,
        // `:nth-col()`, `:nth-last-col()`) is unimplemented in LEXBOR TOO
        // (§D-6's `default:` case) - a real, agreed "always false", not a
        // gap this port introduces.
        Simple::PseudoClassFunction(FunctionArg::Other) => SimpleCheck::Result(false),
        Simple::PseudoElement | Simple::Other => SimpleCheck::Result(false),
    })
}

/// A simple selector's name resolved against the document of the first
/// candidate it is tested on, and kept for the rest of the query - Lexbor's
/// own lazily set `entry->id` - for the kinds that look a name up on every
/// candidate. Lexbor keys element and attribute names by their ASCII
/// lower-cased form, so an id match is exactly the case-folded comparison
/// the byte path makes (a type selector), or a necessary condition that the
/// adapter then confirms (an attribute: `attr_by_resolved_name`). A
/// candidate from another document is compared by name.
#[derive(Clone, Copy, Default)]
enum Name<'doc> {
    /// Not looked up yet.
    #[default]
    Unresolved,
    /// Nothing to look up: not a type or attribute selector.
    None,
    /// A type selector's tag id in that document; `None`: no element of the
    /// document has the name.
    Tag(HtmlDoc<'doc>, Option<TagId>),
    /// An attribute selector's name.
    Attr(AttrName),
}

impl<'doc> Name<'doc> {
    fn resolve(sel: &Step<'_>, doc: HtmlDoc<'doc>) -> Name<'doc> {
        match sel.simple {
            Simple::Type => Name::Tag(doc, doc.tag_id(sel.name)),
            Simple::Attribute(_) => Name::Attr(doc.resolve_attr_name(sel.name)),
            _ => Name::None,
        }
    }
}

/// A query's [`Name`]s, by simple-selector index: one per simple
/// selector, each resolved when it is first reached.
type Names<'doc> = Table<Name<'doc>>;

/// §B-1 through [`Name`]: one id comparison where the document is the one
/// the name was resolved in, [`name_eq`] otherwise.
#[inline]
fn type_matches(node: HtmlNode<'_>, want: &[u8], name: Name<'_>) -> bool {
    match name {
        Name::Tag(doc, id) if node.owner_document() == doc => {
            node.element().is_some() && id.is_some() && node.tag_id() == id
        }
        _ => name_eq(node, want),
    }
}

/// The value of `node`'s attribute `qname` (DOM `getAttribute`), through
/// the resolved [`Name`] when there is one.
#[inline]
fn attr_value<'doc>(node: HtmlNode<'doc>, qname: &[u8], name: Name<'_>) -> Option<&'doc [u8]> {
    let el = node.element()?;
    match name {
        Name::Attr(resolved) => el
            .attr_by_resolved_name(qname, resolved)
            .map(HtmlAttr::value),
        _ => el.get_attribute(qname),
    }
}

/* ------------------------------------------------------------------ *
 * the task stack - THE point of this module                           *
 * ------------------------------------------------------------------ */

/// A range of [`Compiled::alts`]: one nested list's alternatives.
#[derive(Clone, Copy)]
struct Alts {
    next: u32,
    end: u32,
}

/// Match `chain` right to left - Lexbor's `lxb_selectors_state_find` /
/// `found_check` / `not_found` over its entries: compound `idx` at `cur`,
/// its simple selectors from `rest` on. `at[at_base + i]` is where compound
/// `i` currently stands, which is what a backtrack moves on.
#[derive(Clone, Copy)]
struct ChainTask<'doc> {
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
struct ForwardTask<'doc> {
    chain: Chain,
    level: u32,
    cand: HtmlNode<'doc>,
    rest: u32,
    cur_base: u32,
}

/// `of S` (§D-1): is `node` in `S` (`counting == false`), then how many of
/// its siblings in the counting direction are - `pos` so far.
#[derive(Clone, Copy)]
struct NthOfTask<'doc> {
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
enum Task<'doc> {
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
struct Query<'c, 'p, 'doc> {
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
    fn new(
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
    fn finish(self, scratch: &mut Scratch) {
        self.names.give_back(&mut scratch.names);
        scratch.tasks = recycle(self.tasks, SCRATCH_KEEP);
        scratch.at = recycle(self.at, SCRATCH_KEEP);
        scratch.cursors = recycle(self.cursors, SCRATCH_KEEP);
    }

    /// Does `node` match any top-level alternative?
    fn matches_top(&mut self, node: HtmlNode<'doc>) -> Result<bool, MatchFailure> {
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

/* ------------------------------------------------------------------ *
 * whole-query entry points: matches_any / select_all / select_first,  *
 * over a full (possibly comma-separated) `Lists` - what               *
 * `Node#{matches?,css,at_css}` each want. Each compiles first         *
 * ([`compile`]), so a selector it refuses is refused before any node   *
 * is matched, whatever the document holds.                            *
 * ------------------------------------------------------------------ */

/// Does `element` match any comma-separated alternative of `groups`? The
/// entry point for `Node#matches?`: no traversal, under a fresh budget.
pub fn matches_any(groups: Lists<'_>, element: HtmlElement<'_>) -> Result<bool, MatchFailure> {
    matches_any_in(&mut Scratch::new(), groups, element)
}

/// [`matches_any`], over `scratch`'s stacks.
pub fn matches_any_in(
    scratch: &mut Scratch,
    groups: Lists<'_>,
    element: HtmlElement<'_>,
) -> Result<bool, MatchFailure> {
    let compiled = compile_in(scratch, groups)?;
    let answer = Query::new(&compiled, None, DEFAULT_WORK_BUDGET, scratch).and_then(|mut query| {
        let answer = query.matches_top(element.node());
        query.finish(scratch);
        answer
    });
    compiled.give_back(scratch);
    answer
}

/// Why [`select_all`]/[`select_first`]/[`matches_any`] stopped before
/// answering the whole query. [`select_all`]'s own `Overflow` plus whatever
/// [`MatchFailure`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFailure {
    /// More descendants matched than a Makiri result set is allowed to hold.
    Overflow,
    /// See [`MatchFailure::WorkExceeded`].
    WorkExceeded,
    /// See [`MatchFailure::Unsupported`].
    Unsupported,
    /// See [`MatchFailure::TooComplex`].
    TooComplex,
    /// See [`MatchFailure::Oom`].
    Oom,
}

impl From<MatchFailure> for QueryFailure {
    #[inline]
    fn from(e: MatchFailure) -> Self {
        match e {
            MatchFailure::WorkExceeded => QueryFailure::WorkExceeded,
            MatchFailure::Unsupported => QueryFailure::Unsupported,
            MatchFailure::TooComplex => QueryFailure::TooComplex,
            MatchFailure::Oom => QueryFailure::Oom,
        }
    }
}

/// Every ELEMENT in `root`'s subtree - descendants only, `root` itself
/// excluded, exactly as the Lexbor-backed engine's `find` does (see
/// `lexbor::selectors::find_cb`) - that matches any alternative of `groups`,
/// in document order. A node matching more than one comma alternative is
/// reported once: the top-level alternatives are a plain OR for ONE node,
/// so no `MATCH_FIRST`-style dedup flag is needed the way Lexbor's C API
/// wants one.
///
/// `<template>` contents are not entered (`preorder_next`, not
/// `preorder_next_with_contents`) - the DOM's own rule for a descendant
/// walk, which `children`/`content=` already follow.
///
/// Capped at [`NODE_SET_MAX`], matching every other Makiri result set, and
/// at one shared work budget for the whole call (`Budget`'s doc).
pub fn select_all<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    select_all_in(&mut Scratch::new(), root, groups)
}

/// [`select_all`], over `scratch`'s stacks.
pub fn select_all_in<'doc>(
    scratch: &mut Scratch,
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    select_all_with_limit(scratch, root, groups, DEFAULT_WORK_BUDGET)
}

fn select_all_with_limit<'doc>(
    scratch: &mut Scratch,
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
    limit: u64,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    let compiled = compile_in(scratch, groups)?;
    let found = select_all_compiled(scratch, &compiled, root, limit);
    compiled.give_back(scratch);
    found
}

fn select_all_compiled<'doc>(
    scratch: &mut Scratch,
    compiled: &Compiled<'_>,
    root: HtmlNode<'doc>,
    limit: u64,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    let mut query = Query::new(compiled, Some(root.owner_document()), limit, scratch)?;
    let mut walk = || {
        let mut out = Vec::new();
        let mut n = root;
        while let Some(next) = n.preorder_next(root) {
            n = next;
            if n.element().is_some() && query.matches_top(n)? {
                if out.len() >= NODE_SET_MAX {
                    return Err(QueryFailure::Overflow);
                }
                out.falloc_push(n).map_err(|()| QueryFailure::Oom)?;
            }
        }
        Ok(out)
    };
    let found = walk();
    query.finish(scratch);
    found
}

/// Test-only: [`select_all`] with a caller-chosen work-budget limit instead
/// of [`DEFAULT_WORK_BUDGET`], so a test can prove the budget actually stops
/// an expensive `:has()` search without a document big enough to exhaust the
/// real, shipped limit.
#[cfg(test)]
pub(crate) fn select_all_with_work_limit<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
    limit: u64,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    select_all_with_limit(&mut Scratch::new(), root, groups, limit)
}

/// The first descendant of `root`, in document order, that matches any
/// alternative of `groups` - `root` itself excluded. Stops at the first hit
/// instead of building the whole set, as `Node#at_css` wants (see
/// `lexbor::selectors::first_cb`). One work budget for the whole search.
pub fn select_first<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
    select_first_in(&mut Scratch::new(), root, groups)
}

/// [`select_first`], over `scratch`'s stacks.
pub fn select_first_in<'doc>(
    scratch: &mut Scratch,
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
    let compiled = compile_in(scratch, groups)?;
    let found = select_first_compiled(scratch, &compiled, root);
    compiled.give_back(scratch);
    found
}

fn select_first_compiled<'doc>(
    scratch: &mut Scratch,
    compiled: &Compiled<'_>,
    root: HtmlNode<'doc>,
) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
    let mut query = Query::new(
        compiled,
        Some(root.owner_document()),
        DEFAULT_WORK_BUDGET,
        scratch,
    )?;
    let mut walk = || {
        let mut n = root;
        while let Some(next) = n.preorder_next(root) {
            n = next;
            if n.element().is_some() && query.matches_top(n)? {
                return Ok(Some(n));
            }
        }
        Ok(None)
    };
    let found = walk();
    query.finish(scratch);
    found
}

#[cfg(test)]
mod scratch_tests {
    use super::*;
    use crate::falloc::Reserve;

    /// [`recycle`] carries the allocation across the lifetime change - the
    /// point of [`Scratch`] - and drops one past its keep limit.
    #[test]
    fn recycle_keeps_the_allocation() {
        let byte = 7u8;
        let mut v: Vec<Option<&u8>> = Vec::new();
        v.falloc_reserve(16).expect("reserve");
        v.falloc_push(Some(&byte)).expect("push");
        let (ptr, cap) = (v.as_ptr() as usize, v.capacity());
        let w: Vec<Option<&'static u8>> = recycle(v, usize::MAX);
        assert!(w.is_empty());
        assert_eq!((w.as_ptr() as usize, w.capacity()), (ptr, cap));
        let dropped: Vec<Option<&'static u8>> = recycle(w, 4);
        assert_eq!(dropped.capacity(), 0);
    }
}

#[cfg(test)]
mod anb_tests {
    use super::*;

    /// The `n >= 0` definition, by search: the reference for small values.
    fn by_definition(a: i64, b: i64, pos: u64) -> bool {
        (0..=64i64).any(|n| i128::from(a) * i128::from(n) + i128::from(b) == i128::from(pos))
    }

    #[test]
    fn small_values_follow_the_definition() {
        for a in -5i64..=5 {
            for b in -12i64..=12 {
                for pos in 1u64..=30 {
                    assert_eq!(
                        anb_matches(a as c_long, b as c_long, pos),
                        by_definition(a, b, pos),
                        "{a}n{b:+} at {pos}"
                    );
                }
            }
        }
    }

    /// The extremes Lexbor's parser clamps to, which overflowed 64 bits
    /// (and panicked in a release build) before.
    #[test]
    fn extreme_values_answer_without_overflow() {
        let (max, min) = (c_long::MAX, c_long::MIN);
        for pos in [1u64, 2, 3, 1 << 40, u64::MAX] {
            // n - MAX: every position (n = pos + MAX).
            assert!(anb_matches(1, -max, pos), "n-MAX at {pos}");
            assert!(anb_matches(1, min, pos), "n+MIN at {pos}");
            // -n + MIN / -n - MAX: none (n would be negative).
            assert!(!anb_matches(-1, min, pos));
            assert!(!anb_matches(-1, -max, pos));
            // MAX n + MAX, MIN n + MIN: none of these positions.
            assert!(!anb_matches(max, max, pos) || pos == max as u64);
            assert!(!anb_matches(min, min, pos));
        }
        // 2n - MAX: MAX is odd, so exactly the odd positions.
        assert!(anb_matches(2, -max, 1));
        assert!(!anb_matches(2, -max, 2));
        assert!(anb_matches(2, -max, 3));
        // -n + MAX: every position up to MAX.
        assert!(anb_matches(-1, max, 1));
        assert!(anb_matches(-1, max, max as u64));
        assert!(!anb_matches(-1, max, max as u64 + 1));
        // a = 0 with an extreme b: never a position.
        assert!(!anb_matches(0, min, 1));
        assert!(!anb_matches(0, max, 1));
        assert!(anb_matches(0, 3, 3));
    }
}
