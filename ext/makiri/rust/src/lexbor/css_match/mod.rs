//! CSS selector matcher over the typed HTML adapter, structured as an
//! explicit (heap) work stack rather than native recursion - the same
//! architectural property Lexbor's own `lxb_selectors_*` state machine has
//! (`lxb_selectors_run`'s loop,
//! `do { entry = selectors->state(...) } while (entry != NULL);`, never
//! recurses on selector nesting). Selector PARSING is not reimplemented: it
//! goes through
//! the existing `lexbor::css_parser` typed view, the same one the XML
//! CSS->XPath lowering already uses.
//!
//! Wired into `Node#css`/`#at_css`/`#matches?` (`glue::html_node::css`). A
//! port was chosen over adopting the `selectors`/`cssparser` crates, which
//! were explored and discarded: their matcher recurses natively on selector
//! nesting ("Why an explicit stack" below). The spike code is gone, findable
//! only through git history.
//!
//! # This is a semantic port of Lexbor's `selectors.c`, not a code port
//!
//! Every matching rule here is read off
//! `vendor/lexbor/source/lexbor/selectors/selectors.c` and its test suite
//! (`vendor/lexbor/test/lexbor/selectors/selectors.c`), and each names the
//! Lexbor function or test it follows (`lxb_selectors_match_element`,
//! `match_id_class_case`, ...), so it can be checked against the vendored
//! source rather than against a summary of it.
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
//!   read - `lxb_selectors_pseudo_class_disabled`), `option`/`optgroup`
//!   count, `:enabled` is only for the elements `:disabled` is defined for
//!   (Lexbor: any element), and an
//!   element with a custom tag is neither `:disabled` nor `:checked` by its
//!   attribute alone. `:read-write` / `:read-only` ask the same
//!   `is_disabled`, so an `<input>` in a `<fieldset disabled>` is read-only
//!   here and read-write to Lexbor. Checked against the Standard
//!   (`form_state_pseudo_classes_follow_the_html_standard`); the Lexbor
//!   differential fuzzer leaves all five out.
//! - `lxb_selectors_anb_calc` tests `:nth-*`'s `An+B` with a `double`
//!   division, which past 2^53 answers "divisible" for everything; this port
//!   computes exactly, in `i128` (`anb_matches`'s doc).
//! - `:nth-child(An+B of S)` / `:nth-last-child(An+B of S)` count by
//!   the CSS definition - an element in `S`, ranked among its element
//!   siblings that are in `S` - where Lexbor miscounts in many shapes: a
//!   comma list in `S` (it starts from the LAST list, `anb->of->last`, so
//!   `p:nth-child(2 of ul, p)` and `(2 of p, ul)` answer differently), a
//!   combinator in `S` (`span:nth-child(2 of li span)` finds a `<span>` that
//!   is first among `li span` siblings), and even simple pseudos such as
//!   `:enabled` or `:empty` in `S`, or the ORDER of simple selectors in its
//!   compound (`[data-n^='1']:nth-child(odd)` vs `:nth-child(odd)[data-n^='1']`).
//!   This port's answer is checked against a spec oracle rather than Lexbor
//!   (`lexbor::tests::css_match::nth_child_of_s_agrees_with_a_spec_oracle`);
//!   the Lexbor differential fuzzer leaves `of S` out. The walk itself is
//!   Lexbor's shape - candidate in `S`, then siblings one at a time
//!   (`NthOfTask`) on the task stack.
//! - An attribute selector's NAME is looked up as the DOM's `getAttribute`
//!   does - ASCII case-insensitive on an HTML element in an HTML document,
//!   case-sensitive otherwise, the HTML Standard's rule for Selectors - where
//!   Lexbor folds case everywhere (so `[viewbox]` finds an SVG `viewBox`
//!   there, not here) - and by QUALIFIED name, so `[href]` does not find a
//!   prefixed `xlink:href`, which Lexbor takes by its local name (Selectors:
//!   a name without a namespace prefix means no namespace). Resolving names
//!   to Lexbor's ids ([`Name`]) keeps this rule exactly (an id match is only
//!   a pre-filter); `lexbor::tests::css_match::resolved_names_agree_with_the_old_engine`
//!   pins that case is the only difference for an unprefixed attribute.
//!   The pseudo-classes Lexbor answers from an attribute's presence
//!   (`:any-link`, `:required`, `:placeholder-shown`, `:hover`, ...) keep
//!   Lexbor's local-name rule, `xlink:href` included.
//! - A compound that STARTS with `:is()` / `:where()` / `:not()` / `:has()`
//!   is tried at every candidate its combinator allows - every ancestor for
//!   a descendant combinator, every preceding sibling for `~`, and in a
//!   `:has()` argument every child, descendant or following sibling - where
//!   Lexbor tries only the first: `:is(div) span` misses a `<span>` whose
//!   parent is a `<p>` in a `<div>`, and `:has(:has(a))` misses `<html>`.
//!   (`*:is(div) span` is answered correctly there.) Found by the
//!   `html_css_diff` fuzz target; pinned by
//!   `lexbor::tests::css_match::a_compound_led_by_a_list_pseudo_tries_every_candidate`,
//!   and the differential checks leave the shape out.
//! - `#id` / `.class` read the DOM's ID and class attributes - the
//!   no-namespace `id` / `class` - through Lexbor's shortcut, as Lexbor
//!   does; a lookup by qualified name would also take an unprefixed `id` set
//!   IN a namespace (`setAttributeNS("urn:x", "id")`), which is not the ID.
//!
//! Still open, and never a silent wrong answer: `::pseudo-elements` (never
//! match), `:lexbor-contains()` (raised - a Lexbor extension, not CSS,
//! deliberately not reimplemented; see `CHANGELOG.md`), `:current()`
//! (deferred, not ruled out). Closed,
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
//! depth of only ~2,000-2,500 in a release build). The task stack (`query`)
//! avoids that: nesting depth becomes heap growth, never call-stack growth -
//! verified at 500,000 levels in `lexbor::tests::css_match`.
//!
//! `:has()`'s own forward search (Lexbor's `*_forward` states,
//! `lxb_selectors_state_found_check_forward` and the rest) is heap-based
//! too, for the same reason and the same way: `ForwardTask`/`HasCursor` walk candidates and
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
//! recursion (`lxb_selectors_run`'s loop, above) - the
//! fix here is bringing `:has()` in line with what Lexbor (and this module's
//! own `:is()`/`:where()`/`:not()`) already do, not inventing a new
//! technique. `MAX_COMPOUNDS` still bounds one `:has()` chain's length (a
//! sanity cap, not a stack-safety one - see its doc), but nesting depth is
//! now unbounded the same way `:is()` nesting is, verified the same way (a
//! stress test at a depth far beyond the crash threshold, on a
//! `RUBY_FIBER_MACHINE_STACK_SIZE`-sized thread stack, in
//! `lexbor::tests::css_match`).

#![forbid(unsafe_code)]

mod compile;
mod positions;
mod query;
mod scratch;
mod simple;
mod state;
mod tree;

pub use compile::check_compiles;
#[cfg(test)]
pub(crate) use compile::validate;
pub use scratch::Scratch;

use crate::falloc::VecPush;
use crate::lexbor::adapter::html::{HtmlElement, HtmlNode};
use crate::lexbor::css_parser::Lists;
use crate::limits::NODE_SET_MAX;

use compile::{compile, Compiled};
use query::Query;

/// A complexity bound on compounds per chain, mirroring `css::MAX_COMPOUNDS`.
///
/// Not a stack-safety mechanism (this design needs none, for `:has()` nesting
/// or any other - see the module doc) - just a sanity cap so a single
/// compound chain (either the main match or one `:has()` alternative) can't
/// grow unboundedly.
pub(crate) const MAX_COMPOUNDS: usize = 64;

/// The per-query work budget's default cap - the count [`Budget::charge`]
/// compares against: the total number of steps one top-level call
/// ([`matches_any`], [`select_all`], [`select_first`]) may take. The same
/// 50M as XPath's `max_eval_ops`; a step is a few nanoseconds, so this is a
/// few hundred milliseconds of matching. It has to be far above what an
/// ordinary query costs, which is not the document's size: an unmatched
/// `.x ~ li` over n siblings tests n^2/2 compounds, `.none p` candidates
/// times depth.
const DEFAULT_WORK_BUDGET: u64 = 50 * 1000 * 1000;

/// One top-level call's work budget: every step charges it once - each
/// compound tested, each `:has()` candidate visited, each alternative tried
/// and each sibling `:nth-*` counts. Plain chains are charged too, not only
/// `:has()` and `:nth-*`: their backtracking is bounded by pruning
/// (`Query::step_chain`), not by the document's size. Exceeding it is
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
    /// either cannot run (the column combinator `||`, `Combinator::Column` -
    /// Lexbor's own traversal reports an error status for it too) or
    /// implements and this port deliberately does not
    /// (`:lexbor-contains()`, `FunctionArg::Contains` - a Lexbor extension,
    /// not CSS; `CHANGELOG.md` records the removal). Answering
    /// `false` for either would be indistinguishable from "genuinely no
    /// element satisfies this", which it is not.
    Unsupported,
    /// A chain somewhere in the selector has more than [`MAX_COMPOUNDS`]
    /// compounds - see [`compile()`](compile::compile)'s doc for why this is caught up front
    /// rather than left to a silent never-matching chain.
    TooComplex,
    /// An allocation the match needed failed (`falloc`). Raised, like every
    /// other failure here, never answered as a partial result.
    Oom,
    /// A broken invariant of the matcher itself - a table lookup out of range,
    /// a task for a step that cannot defer - never a property of the selector.
    /// Raised as `Makiri::InternalError` rather than as `Unsupported`'s
    /// "selector could not be run": the selector is not at fault, and a bare
    /// `rescue` must not take a matcher bug for a bad selector.
    Internal,
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

/* ------------------------------------------------------------------ *
 * whole-query entry points: matches_any / select_all / select_first,  *
 * over a full (possibly comma-separated) `Lists` - what               *
 * `Node#{matches?,css,at_css}` each want. Each compiles first         *
 * ([`compile`]), so a selector it refuses is refused before any node   *
 * is matched, whatever the document holds.                            *
 * ------------------------------------------------------------------ */

/// Does `element` match any comma-separated alternative of `groups`? The
/// entry point for `Node#matches?`: no traversal, under a fresh budget, over
/// `scratch`'s tables and stacks.
pub fn matches_any(
    scratch: &mut Scratch,
    groups: Lists<'_>,
    element: HtmlElement<'_>,
) -> Result<bool, MatchFailure> {
    let compiled = compile(scratch, groups)?;
    let answer = Query::new(&compiled, None, DEFAULT_WORK_BUDGET, scratch).and_then(|mut query| {
        let answer = query.matches_top(element.node());
        query.finish(scratch);
        answer
    });
    compiled.give_back(scratch);
    answer
}

/// Why [`select_all`] stopped before answering the whole query: the one
/// failure only a result SET can have, or any failure matching can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFailure {
    /// More descendants matched than a Makiri result set is allowed to hold.
    Overflow,
    /// Matching failed ([`select_first`] and [`matches_any`] report these
    /// alone).
    Match(MatchFailure),
}

impl From<MatchFailure> for QueryFailure {
    #[inline]
    fn from(e: MatchFailure) -> Self {
        QueryFailure::Match(e)
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
/// at one shared work budget for the whole call (`Budget`'s doc). The
/// tables and stacks are `scratch`'s.
pub fn select_all<'doc>(
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
    let compiled = compile(scratch, groups)?;
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
                out.falloc_push(n)
                    .map_err(|()| QueryFailure::Match(MatchFailure::Oom))?;
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
/// `lexbor::selectors::first_cb`). One work budget for the whole search,
/// over `scratch`'s tables and stacks.
pub fn select_first<'doc>(
    scratch: &mut Scratch,
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
    let compiled = compile(scratch, groups)?;
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
