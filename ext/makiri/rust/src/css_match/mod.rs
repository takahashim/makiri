//! CSS selector matcher over any DOM that implements [`dom`]'s traits - the
//! HTML arena's are `html::dom::css` - structured as an
//! explicit (heap) work stack rather than native recursion - the same
//! architectural property Lexbor's own `lxb_selectors_*` state machine has
//! (`lxb_selectors_run`'s loop,
//! `do { entry = selectors->state(...) } while (entry != NULL);`, never
//! recurses on selector nesting). Selector PARSING is the parser port's
//! (`crate::selectors`), read through its typed views.
//!
//! Wired into `Node#css`/`#at_css`/`#matches?` on both representations
//! (`glue::html_node::css`, `glue::xml_node::css`). A
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
//!   attribute alone. `:read-write` / `:read-only` are the Standard's too
//!   (M9): an `input` the `readonly` attribute applies to, a `textarea`, an
//!   editable element - Lexbor takes every `input`, and no editable one.
//!   Checked against the Standard
//!   (`form_state_pseudo_classes_follow_the_html_standard`); the Lexbor
//!   differential fuzzer leaves all five out.
//! - `:any-link` / `:link` are an HTML `a` / `area` or an SVG `a` with an
//!   `href` (M2), where Lexbor takes `link` and `map` in any namespace, and
//!   `:empty` counts element and non-empty text children only (M10), where
//!   Lexbor counts every child but a comment. Both as the browsers answer.
//!   The harnesses that hold this matcher to Lexbor's engine run Lexbor's
//!   rules (`compat::as_lexbor`).
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
//! - In a `:has()` argument, `L + R` is looked for over every pair, where
//!   Lexbor stops after the first pair that fails a list pseudo-class
//!   (`:is()`, `:not()`, `:where()`, `:has()`) in `R`'s compound - unless a
//!   type selector in front of it refused that pair first:
//!   `span:has(* + :not(li))` misses `<span><i></i><li></li><a></a></span>`,
//!   whose first pair is `i + li` (`a:not(li)` is answered correctly). Only
//!   for `+`. Found by the randomized differential check;
//!   pinned by
//!   `lexbor::tests::css_match::a_sibling_compound_with_a_list_pseudo_in_has_tries_every_pair`,
//!   and the differential checks leave the shape out.
//! - `#id` / `.class` read the DOM's ID and class attributes - the
//!   no-namespace `id` / `class` - through Lexbor's shortcut, as Lexbor
//!   does; a lookup by qualified name would also take an unprefixed `id` set
//!   IN a namespace (`setAttributeNS("urn:x", "id")`), which is not the ID.
//!
//! Still open, and never a silent wrong answer: `::pseudo-elements` (never
//! match), `:current()`
//! (raised - Selectors 5 §6, where the time-dimensional pseudo-classes went
//! from Level 4, has it match nothing on an element with no timeline, which
//! is every HTML element; Lexbor matches it as `:is()`; and Lexbor's parser
//! refuses the argument-less `:current`/`:past`/`:future`. Refused rather
//! than answered with either). Closed,
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
pub mod dom;
mod lang;
mod ns;
mod nth_of;
mod plan;
mod positions;
mod query;
mod scratch;
mod simple;
mod state;
mod tree;

#[cfg(all(test, feature = "lexbor"))]
pub(crate) use compile::validate;
pub use ns::Namespaces;
pub use scratch::Scratch;

use crate::falloc::VecGrow;
use crate::limits::NODE_SET_MAX;
use crate::selectors::ast::Lists;

use compile::{compile, Compiled};
use core::ops::ControlFlow;
use dom::CssNode;
use query::Query;

/// A complexity bound on compounds per chain. The reference lowering
/// (`crate::css`) is held to the same one.
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
#[cfg(not(fuzzing))]
const DEFAULT_WORK_BUDGET: usize = 50 * 1000 * 1000;
/// Under cargo-fuzz (`--cfg fuzzing`) the cap is lower: `html_css` runs up to
/// ~130 queries per input, and a few that each stay just under 50M - triply
/// nested `:has()` over a 4 KiB document - took 42 s under ASan, past
/// libFuzzer's timeout. Ordinary queries on such a document cost far less,
/// and an expensive one now reaches the refusal path instead.
#[cfg(fuzzing)]
const DEFAULT_WORK_BUDGET: usize = 250 * 1000;

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
/// happened, not a formatted diagnostic, so one `usize` counter local to a
/// single call needs no shared ownership.
struct Budget {
    spent: std::cell::Cell<usize>,
    limit: usize,
    /// What `:scope` is: the element the query was made from. Kept here, with
    /// the other per-query state `check_simple` already takes, so a compound's
    /// test takes no further argument (a scan row read 3-5% slower with one).
    scope: Option<crate::ptr_table::NodeKey>,
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
    /// implements and this port deliberately does not (`:current()`,
    /// `FunctionArg::Current`, which Lexbor matches as `:is()` where
    /// Selectors 5 has it match nothing - the module doc). Answering
    /// `false` for either would be indistinguishable from "genuinely no
    /// element satisfies this", which it is not.
    Unsupported,
    /// A chain somewhere in the selector has more than [`MAX_COMPOUNDS`]
    /// compounds - see [`compile()`](compile::compile)'s doc for why this is caught up front
    /// rather than left to a silent never-matching chain.
    TooComplex,
    /// A namespace prefix in the selector that the query's [`Namespaces`]
    /// do not bind: the selector is invalid (Selectors 4 §6.1), found when
    /// it is compiled.
    UndeclaredPrefix,
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
        let Some(n) = crate::limits::admit(self.spent.get(), 1, self.limit) else {
            return Err(MatchFailure::WorkExceeded);
        };
        self.spent.set(n);
        Ok(())
    }

    #[inline]
    fn scope(&self) -> Option<crate::ptr_table::NodeKey> {
        self.scope
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

/// Does `node` match any comma-separated alternative of `groups`? The
/// entry point for `Node#matches?`: no traversal, under a fresh budget, over
/// `scratch`'s tables and stacks. A node that is not an element matches
/// nothing, once the selector compiles.
pub fn matches_any<'d, N: CssNode<'d>>(
    scratch: &mut Scratch<N::Host>,
    groups: Lists<'_>,
    ns: &Namespaces<'_>,
    node: N,
) -> Result<bool, MatchFailure> {
    with_query(
        scratch,
        groups,
        ns,
        None,
        Some(node),
        DEFAULT_WORK_BUDGET,
        |query| {
            if node.element().is_none() {
                return Ok(false);
            }
            query.matches_top(node)
        },
    )
}

/// What `:scope` is for a query made from `root`: `root` itself when it is
/// an element, the document element when it is a document (as
/// `document.querySelectorAll(':scope')` has it), and nothing for any other
/// node.
fn scope_of<'d, N: CssNode<'d>>(root: N) -> Option<N> {
    if root.element().is_some() {
        return Some(root);
    }
    if root.node_type() != crate::node_type::NodeType::Document {
        return None;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if c.element().is_some() {
            return Some(c);
        }
        child = c.next();
    }
    None
}

/// Compile `groups` ([`compile`]: a selector it refuses is refused before
/// any node is matched), make a query over it - walking `walk_in` when
/// given - and run `f`; every table and stack goes back to `scratch`
/// whatever `f` answers. The one place the entry points borrow and return
/// a [`Scratch`].
fn with_query<'d, N: CssNode<'d>, R, E: From<MatchFailure>>(
    scratch: &mut Scratch<N::Host>,
    groups: Lists<'_>,
    ns: &Namespaces<'_>,
    walk_in: Option<N::Doc>,
    scope: Option<N>,
    limit: usize,
    f: impl FnOnce(&mut Query<'_, '_, 'd, N>) -> Result<R, E>,
) -> Result<R, E> {
    let compiled = compile(&mut scratch.tables, groups, ns)?;
    let answer = match Query::new(&compiled, walk_in, scope, limit, scratch) {
        Ok(mut query) => {
            let answer = f(&mut query);
            query.finish(scratch);
            answer
        }
        Err(e) => Err(e.into()),
    };
    compiled.give_back(&mut scratch.tables);
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
pub fn select_all<'d, N: CssNode<'d>>(
    scratch: &mut Scratch<N::Host>,
    root: N,
    groups: Lists<'_>,
    ns: &Namespaces<'_>,
) -> Result<Vec<N>, QueryFailure> {
    select_all_with_limit(scratch, root, groups, ns, DEFAULT_WORK_BUDGET)
}

fn select_all_with_limit<'d, N: CssNode<'d>>(
    scratch: &mut Scratch<N::Host>,
    root: N,
    groups: Lists<'_>,
    ns: &Namespaces<'_>,
    limit: usize,
) -> Result<Vec<N>, QueryFailure> {
    let walk_in = Some(root.owner_document());
    with_query(
        scratch,
        groups,
        ns,
        walk_in,
        scope_of(root),
        limit,
        |query| {
            let mut out = Vec::new();
            let stopped = each_match(query, root, |n| {
                if out.len() >= NODE_SET_MAX {
                    return ControlFlow::Break(QueryFailure::Overflow);
                }
                match out.falloc_push(n) {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(()) => ControlFlow::Break(QueryFailure::Match(MatchFailure::Oom)),
                }
            })?;
            match stopped {
                Some(failure) => Err(failure),
                None => Ok(out),
            }
        },
    )
}

/// Test-only: [`select_all`] with a caller-chosen work-budget limit instead
/// of [`DEFAULT_WORK_BUDGET`], so a test can prove the budget actually stops
/// an expensive `:has()` search without a document big enough to exhaust the
/// real, shipped limit.
#[cfg(all(test, feature = "lexbor"))]
pub(crate) fn select_all_with_work_limit<'d, N: CssNode<'d>>(
    root: N,
    groups: Lists<'_>,
    limit: usize,
) -> Result<Vec<N>, QueryFailure> {
    select_all_with_limit(&mut Scratch::new(), root, groups, &Namespaces::NONE, limit)
}

/// The first descendant of `root`, in document order, that matches any
/// alternative of `groups` - `root` itself excluded. Stops at the first hit
/// instead of building the whole set, as `Node#at_css` wants (see
/// `lexbor::selectors::first_cb`). One work budget for the whole search,
/// over `scratch`'s tables and stacks.
pub fn select_first<'d, N: CssNode<'d>>(
    scratch: &mut Scratch<N::Host>,
    root: N,
    groups: Lists<'_>,
    ns: &Namespaces<'_>,
) -> Result<Option<N>, MatchFailure> {
    let walk_in = Some(root.owner_document());
    with_query(
        scratch,
        groups,
        ns,
        walk_in,
        scope_of(root),
        DEFAULT_WORK_BUDGET,
        |query| each_match(query, root, ControlFlow::Break),
    )
}

/// Run `f` on each descendant of `root` the walking `query` matches, in
/// document order - its [`Candidates`], each tested unless the index
/// already answers for all - until `f` breaks, with what it broke with.
/// Internal iteration rather than an `Iterator`: the loop stays the one
/// the compiler inlines `f` into, where an iterator's per-item `Result`
/// cost an index-answered `css("li")` 20%.
fn each_match<'d, N: CssNode<'d>, B>(
    query: &mut Query<'_, '_, 'd, N>,
    root: N,
    mut f: impl FnMut(N) -> ControlFlow<B>,
) -> Result<Option<B>, MatchFailure> {
    let candidates = Candidates::of(query.compiled(), root);
    /* Two loops, not one testing `every` per candidate: the index-answered
     * one is a few instructions a node, and sharing a body with the
     * machine's call cost it 16-20% (`css("li")` in XML). */
    if candidates.all_match() {
        for n in candidates {
            if let ControlFlow::Break(b) = f(n) {
                return Ok(Some(b));
            }
        }
    } else {
        for n in candidates {
            if query.matches_top(n)? {
                if let ControlFlow::Break(b) = f(n) {
                    return Ok(Some(b));
                }
            }
        }
    }
    Ok(None)
}

/// The elements a walking query tests, in document order: `root`'s element
/// descendants, or - where the host has an index for the selector's
/// rightmost type selector ([`Compiled::index_key`](compile::Compiled::index_key)) - that index's bucket,
/// which holds every element that can match and no other order.
enum Candidates<'d, N: CssNode<'d>> {
    Walk {
        root: N,
        at: N,
    },
    /// A bucket, and whether the selector is its type selector alone, so
    /// that every element in it matches.
    Index(N::Candidates, bool),
}

impl<'d, N: CssNode<'d>> Candidates<'d, N> {
    fn of(compiled: &Compiled<'_>, root: N) -> Self {
        let Some((local, ns, whole)) = compiled.index_key() else {
            return Candidates::Walk { root, at: root };
        };
        match root.candidates(local, ns) {
            Some(bucket) => Candidates::Index(bucket, whole),
            None => Candidates::Walk { root, at: root },
        }
    }

    /// Whether every candidate matches, with nothing left to test.
    fn all_match(&self) -> bool {
        matches!(self, Candidates::Index(_, true))
    }
}

impl<'d, N: CssNode<'d>> Iterator for Candidates<'d, N> {
    type Item = N;

    #[inline]
    fn next(&mut self) -> Option<N> {
        match self {
            Candidates::Walk { root, at } => {
                while let Some(next) = at.preorder_next(*root) {
                    *at = next;
                    if next.element().is_some() {
                        return Some(next);
                    }
                }
                None
            }
            Candidates::Index(bucket, _) => bucket.next(),
        }
    }
}
