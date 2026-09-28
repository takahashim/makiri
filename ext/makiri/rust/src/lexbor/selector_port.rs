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
//! (`§B-1` etc.). What is NOT carried over is the C file's data structures
//! (an intrusive `prev`/`next`/`following` linked list over a hand-written
//! object pool) - translating those directly would mean `unsafe` Rust, which
//! defeats the point of this migration. The explicit `Frame`/`Cont` stack
//! below is the safe-Rust shape of the SAME algorithm (heap continuations
//! instead of pointers), not a stylistic choice.
//!
//! Three deliberate, documented departures from a byte-for-byte semantic
//! match, plus what is still open:
//!
//! - `lxb_selectors_pseudo_class_disabled`'s `fieldset` inheritance check
//!   dereferences `first_child` unconditionally (`SEL.c:2207-2208`, §C-2) -
//!   a null pointer read on an empty `<fieldset>`. This port treats a
//!   childless fieldset as having no `<legend>`, which is what the HTML
//!   Standard's algorithm implies; the C bug is not reproduced.
//! - `lxb_selectors_anb_calc` (§D-3) tests `:nth-*`'s `An+B` with a `double`
//!   division, which risks float rounding for large indices; this port uses
//!   integer arithmetic instead.
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
//!   (`Frame::NthOfStep`) on the explicit stack.
//!
//! Still open (tracked in the plan, not silent gaps): `::pseudo-elements`,
//! `:lexbor-contains()` (decided not to reimplement), `:current()` (deferred,
//! not ruled out - `notes/css_selectors_crate_migration_plan.ja.md`). Closed,
//! not open: the selector-nesting cap (see the next section), the work budget
//! ([`Budget`]), and allocation - every table, stack and continuation a query
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
//! depth of only ~2,000-2,500 in a release build). The `Frame`/`Cont`
//! machinery below avoids that: nesting depth becomes heap growth, never
//! call-stack growth - verified at 500,000 levels in
//! `lexbor::tests::selector_port_spike`.
//!
//! `:has()`'s own forward search (§A-4) is heap-based too, for the same
//! reason and the same way: `Frame::HasStep`/`HasCursor` walk candidates and
//! `:has()`-inside-`:has()` nesting without ever making a native Rust call
//! that itself recurses. This was NOT the original design - an earlier
//! version answered `:has()` with ordinary Rust recursion (`has_forward`),
//! reasoning that `MAX_COMPOUNDS` (64, a fixed complexity cap already
//! enforced when a chain is compiled) bounded it safely. That bound was real
//! for ONE `:has()` chain's own compound-by-compound depth, but said nothing
//! about `:has()` NESTED inside another `:has()`'s argument: `check_simple`'s
//! `Has` arm called `has_matches` EAGERLY (unlike `:is`/`:where`/`:not`,
//! which already deferred to this file's heap stack), so each nesting level
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
use crate::lexbor::adapter::html::{HtmlElement, HtmlNode, NodeType, NsId};
use crate::lexbor::css_parser::{
    AttrMatch, Combinator, FunctionArg, List, ListPseudo, Lists, Nth, PseudoClass, Selector, Simple,
};
use crate::limits::NODE_SET_MAX;
use crate::ptr_table::PtrMap;

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
/// ([`matches`], [`matches_any`], [`select_all`], [`select_first`]) may take
/// charging it, which is what stops a `:has()` search from multiplying its
/// cost per candidate element into something unbounded by the document's own
/// size.
const DEFAULT_WORK_BUDGET: u64 = 10 * 1000 * 1000;

/// One top-level call's work budget: every step that can cost MORE than the
/// input document/selector's own size bounds already (concretely: each
/// candidate [`Frame::HasStep`]'s search visits, and each frame [`run`]'s
/// trampoline pops) charges it once. Exceeding it is
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

/* ------------------------------------------------------------------ *
 * compiling a selector: every chain, once, into query-local tables    *
 * ------------------------------------------------------------------ */

/// A compound (a `Close`-linked run of simple selectors) plus the combinator
/// that attaches it to the compound BEFORE it (to its left - more
/// ancestor/earlier-sibling-ward) in the written selector.
#[derive(Clone, Copy)]
struct Compound<'p> {
    first: Selector<'p>,
    comb: Combinator,
}

/// A compound chain: `compounds[start .. start + len]` of the query's
/// [`Compiled`] table, left to right as written. `len == 0` is an empty
/// chain, which never matches.
///
/// `Copy`, so a retry that keeps matching the SAME chain against a new
/// ancestor/sibling (`advance`'s `Descendant`/`SubsequentSibling`) carries
/// two integers. It used to be an `Rc<[Compound]>` (and before that an owned
/// `Vec`), neither of which can be built without an allocation that aborts
/// on failure.
#[derive(Clone, Copy, Default)]
struct Chain {
    start: u32,
    len: u32,
}

impl Chain {
    fn len(self) -> usize {
        self.len as usize
    }

    /// `chain[idx]` alone, as a chain of its own - see
    /// [`single_compound_frame`]. `idx < len`, so this stays inside the
    /// table.
    fn only(self, idx: usize) -> Chain {
        Chain {
            start: self.start + idx as u32,
            len: 1,
        }
    }
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
    compounds: Vec<Compound<'p>>,
    /// The top-level comma alternatives, in order.
    top: Vec<Chain>,
    /// Each nested list's chain, by the list's identity ([`List::key`]).
    nested: PtrMap<*const (), Chain>,
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
    let mut c = Compiled {
        compounds: Vec::new(),
        top: Vec::new(),
        nested: PtrMap::new(),
    };
    let mut pending: Vec<Lists<'_>> = Vec::new();
    for list in groups {
        let chain = c.add_chain(list, &mut pending)?;
        c.top.falloc_push(chain).or_oom()?;
    }
    while let Some(lists) = pending.pop() {
        for list in lists {
            let chain = c.add_chain(list, &mut pending)?;
            c.nested
                .insert(list.key(), chain)
                .map_err(|_| MatchFailure::Oom)?;
        }
    }
    Ok(c)
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
        pending: &mut Vec<Lists<'p>>,
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
            let mut sel = first;
            loop {
                match sel.simple() {
                    Simple::PseudoClassFunction(FunctionArg::Contains(_)) => {
                        return Err(MatchFailure::Unsupported)
                    }
                    Simple::PseudoClassFunction(FunctionArg::Selectors { lists, .. }) => {
                        pending.falloc_push(lists).or_oom()?
                    }
                    Simple::PseudoClassFunction(FunctionArg::Nth { anb, .. }) => {
                        if let Some(of_list) = anb.and_then(|a| a.of_list) {
                            pending.falloc_push(of_list).or_oom()?;
                        }
                    }
                    _ => {}
                }
                match sel.next().filter(|n| n.combinator() == Combinator::Close) {
                    Some(n) => sel = n,
                    None => break,
                }
            }
            self.compounds
                .falloc_push(Compound { first, comb })
                .or_oom()?;
            cur = sel.next();
        }
        Ok(Chain { start, len })
    }

    fn compound(&self, chain: Chain, idx: usize) -> Compound<'p> {
        self.compounds[chain.start as usize + idx]
    }

    /// The chain compiled for the nested `list`. One `compile` never saw is
    /// a broken invariant - answered as "could not be run", never as an
    /// empty, never-matching chain.
    fn nested_chain(&self, list: List<'p>) -> Result<Chain, MatchFailure> {
        self.nested.get(list.key()).ok_or(MatchFailure::Unsupported)
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
    target
        .split(|&b| is_lexbor_whitespace(b))
        .any(|tok| !tok.is_empty() && eq_bytes(tok, want, case_insensitive))
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
    node.element()
        .is_some_and(|el| el.dom_local_name().eq_ignore_ascii_case(want))
}

fn get_attr<'doc>(node: HtmlNode<'doc>, name: &[u8]) -> Option<&'doc [u8]> {
    node.element().and_then(|el| el.get_attribute(name))
}

fn has_attr(node: HtmlNode<'_>, name: &[u8]) -> bool {
    node.element()
        .is_some_and(|el| el.get_attribute(name).is_some())
}

/// §B-5: the 40 HTML attributes whose VALUE compares ASCII case-insensitively
/// when no `i`/`s` modifier is written, and only on an HTML-namespace element
/// of an HTML document (checked by the caller, `attribute_case_insensitive`).
fn is_html_ci_attribute(name: &[u8]) -> bool {
    // Compared case-insensitively against the selector's own attribute name
    // spelling (an author can write `[TYPE=x]`), matching the DOM's own
    // by-name attribute lookup convention elsewhere in this codebase.
    const NAMES: &[&[u8]] = &[
        b"accept",
        b"accept-charset",
        b"align",
        b"alink",
        b"axis",
        b"bgcolor",
        b"charset",
        b"checked",
        b"clear",
        b"codetype",
        b"color",
        b"compact",
        b"declare",
        b"defer",
        b"dir",
        b"direction",
        b"disabled",
        b"enctype",
        b"face",
        b"frame",
        b"hreflang",
        b"http-equiv",
        b"lang",
        b"language",
        b"link",
        b"media",
        b"method",
        b"multiple",
        b"nohref",
        b"noresize",
        b"noshade",
        b"nowrap",
        b"readonly",
        b"rel",
        b"rev",
        b"rules",
        b"scope",
        b"scrolling",
        b"selected",
        b"shape",
        b"target",
        b"text",
        b"type",
        b"valign",
        b"valuetype",
        b"vlink",
    ];
    NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// §B-5: whether `node`'s attribute `name` compares case-insensitively by
/// DEFAULT (no explicit `i`/`s` modifier written).
fn attribute_case_insensitive_by_default(node: HtmlNode<'_>, name: &[u8]) -> bool {
    // §B-5's condition is "element is HTML-namespace AND owner document is
    // an HTML document" - the second half needs a raw document-type read
    // this `#![forbid(unsafe_code)]` module cannot make (see `attrs.rs`'s
    // private `is_html_in_html_doc`). It is dropped rather than
    // approximated: this matcher is only ever invoked for
    // `Makiri::HTML` documents (XML's CSS query goes through `css::lower`
    // instead - see the plan §2), so "owner document is HTML" always holds
    // in practice, and `is_html_namespace` alone correctly tells HTML
    // elements from foreign (SVG/MathML) content within one.
    is_html_namespace(node) && is_html_ci_attribute(name)
}

/// §B-4: `[name op value]` (or `[name]`, existence, when `at.value` is
/// `None`). `Attribute::case_insensitive` is the selector's own `i`/`s`; when
/// neither was written, `attribute_case_insensitive_by_default` supplies the
/// HTML table default.
fn attribute_matches(
    node: HtmlNode<'_>,
    name: &[u8],
    op: AttrMatch,
    at_value: Option<&[u8]>,
    explicit_ci: Option<bool>,
) -> bool {
    let Some(value) = get_attr(node, name) else {
        return false;
    };
    let Some(want) = at_value else {
        return true; // `[name]`: existence only (§B-4)
    };
    let ci = explicit_ci.unwrap_or_else(|| attribute_case_insensitive_by_default(node, name));
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

/// `node`'s 1-based position among its (`from_end`-directed) siblings,
/// counting only same-type ones when `of_type`. O(siblings), uncached (the
/// plan's `NthIndexCache`-equivalent optimization is still open - see the
/// module doc) - but every sibling visited charges `budget`, so a wide
/// sibling list under repeated `:nth-of-type`-family checks costs the SAME
/// budget `:has()`'s search does, not an uncounted O(siblings) per check.
fn sibling_position(
    node: HtmlNode<'_>,
    from_end: bool,
    of_type: bool,
    budget: &Budget,
) -> Result<u64, MatchFailure> {
    // Element-only would agree for `of_type` (a non-element never satisfies
    // `name_matches_type`) but is WRONG for the plain (`of_type == false`)
    // case - see `counts_toward_child_position`'s doc. One walk serves both,
    // since `same_type` already answers `false` for a non-element itself.
    let same_type = |n: HtmlNode<'_>| !of_type || name_matches_type(n, node);
    let mut pos: u64 = 1;
    let mut cur = if from_end {
        next_position_sibling(node)
    } else {
        prev_position_sibling(node)
    };
    while let Some(n) = cur {
        budget.charge()?;
        if same_type(n) {
            pos += 1;
        }
        cur = if from_end {
            next_position_sibling(n)
        } else {
            prev_position_sibling(n)
        };
    }
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

/// §C-1 `:any-link`/`:link`: `a`/`area`/(`:link` only) `link`, with an `href`.
fn is_any_link(node: HtmlNode<'_>, include_link_tag: bool) -> bool {
    is_html_namespace(node)
        && node.element().is_some_and(|el| {
            let n = el.dom_local_name();
            n == b"a" || n == b"area" || (include_link_tag && n == b"link")
        })
        && has_attr(node, b"href")
}

/// §C-2 `lxb_selectors_pseudo_class_disabled`, WITHOUT the C implementation's
/// null-pointer read on a childless `<fieldset>` (module doc).
fn is_disabled(node: HtmlNode<'_>) -> bool {
    let Some(el) = node.element() else {
        return false;
    };
    let local = el.dom_local_name();
    let is_form_control = matches!(local, b"button" | b"input" | b"select" | b"textarea");
    if !is_form_control {
        return false;
    }
    if has_attr(node, b"disabled") {
        return true;
    }
    // Inherit from an enclosing <fieldset disabled> - unless this element is
    // (or comes after) that fieldset's own <legend>.
    let mut ancestor = parent_element(node);
    while let Some(a) = ancestor {
        if a.element()
            .is_some_and(|e| e.dom_local_name() == b"fieldset")
            && has_attr(a, b"disabled")
        {
            // The HTML Standard: a descendant of the fieldset's first
            // <legend> is exempt; anything else (including a childless
            // fieldset, which safely has no legend) is disabled.
            let first_legend = a
                .first_child()
                .filter(|c| c.element().is_some_and(|e| e.dom_local_name() == b"legend"));
            if let Some(legend) = first_legend {
                let mut n = Some(node);
                while let Some(cur) = n {
                    if cur == legend {
                        return false;
                    }
                    n = cur.parent();
                }
            }
            return true;
        }
        ancestor = parent_element(a);
    }
    false
}

fn is_checked(node: HtmlNode<'_>) -> bool {
    let Some(el) = node.element() else {
        return false;
    };
    let local = el.dom_local_name();
    if local == b"option" {
        return has_attr(node, b"selected");
    }
    if local == b"input" {
        let ty = get_attr(node, b"type");
        let is_checkable = ty.is_some_and(|t| {
            t.eq_ignore_ascii_case(b"checkbox") || t.eq_ignore_ascii_case(b"radio")
        });
        return is_checkable && has_attr(node, b"checked");
    }
    // §C-1: an unknown/custom element also honours a literal `checked`.
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
        PseudoClass::FirstOfType => sibling_position(node, false, true, budget)? == 1,
        PseudoClass::LastOfType => sibling_position(node, true, true, budget)? == 1,
        PseudoClass::OnlyOfType => {
            sibling_position(node, false, true, budget)? == 1
                && sibling_position(node, true, true, budget)? == 1
        }
        PseudoClass::AnyLink => is_any_link(node, false),
        PseudoClass::Link => is_any_link(node, true),
        PseudoClass::Blank => is_blank(node),
        PseudoClass::Checked => is_checked(node),
        PseudoClass::Disabled => is_disabled(node),
        // §C-1: unconditional `!disabled`, NOT gated to form fields the way
        // `:optional`/`:required` are (`SEL.c:1776-1777`) - a plain `<div>`
        // is `:enabled` too, faithfully reproducing Lexbor here.
        PseudoClass::Enabled => !is_disabled(node),
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
) -> Result<bool, MatchFailure> {
    let Some(anb) = anb else {
        return Ok(false);
    };
    let pos = sibling_position(node, from_end, of_type, budget)? as i64;
    Ok(anb_matches(anb, pos))
}

/// §D-3 `lxb_selectors_anb_calc`, done with integer arithmetic instead of
/// Lexbor's `double` division (module doc: avoids float-rounding risk for
/// large indices).
fn anb_matches(anb: crate::lexbor::css_parser::Nth<'_>, pos: i64) -> bool {
    if anb.a == 0 {
        return anb.b >= 0 && pos == anb.b;
    }
    let k = pos - anb.b;
    k % anb.a == 0 && k / anb.a >= 0
}

/* ------------------------------------------------------------------ *
 * :has() - §A-4: heap-based forward search (module doc)              *
 * ------------------------------------------------------------------ */

/// A resumable position in `:has()`'s forward search for ONE compound step -
/// the heap state [`Frame::HasStep`] carries so a `:has()` argument's
/// candidate search (potentially many candidates, unlike the main matcher's
/// single-path ancestor/sibling climbs) never recurses natively, whatever the
/// combinator - the same property `Frame`/`Cont` already give `:is`/`:where`/
/// `:not` nesting (module doc). Built once per compound step
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
    /// `run`'s own per-frame-pop charge (its doc) is what bills `budget` for
    /// each one, exactly as the original `has_forward`'s `candidate_ok`
    /// closure charged once per candidate visited.
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

/// The verdict for one simple selector (or a run of them), or a request to
/// defer to the explicit stack: a `:is`/`:where`/`:not` was hit, and `rest`
/// is whatever `Close`-linked simple selectors of the SAME compound remain
/// after it (checked, eagerly again, once the deferred verdict is back -
/// see [`check_from`]).
enum SimpleCheck<'p> {
    Result(bool),
    Defer {
        negate: bool,
        lists: Lists<'p>,
        rest: Option<Selector<'p>>,
    },
    /// `:has()`: deferred to a heap-based forward search FROM `node` (see
    /// `Frame::HasStep`'s doc) - kept distinct from `Defer` because `:has()`'s
    /// alternatives are resolved by SEARCHING from `node`, not by matching AT
    /// `node` the way `:is`/`:where`/`:not`'s `Defer` alternatives are.
    Has {
        lists: Lists<'p>,
        rest: Option<Selector<'p>>,
    },
    /// `:nth-*(an+b of S)` (§D-1): deferred to `Cont::NthOfSelf`/
    /// `Frame::NthOfStep` - does `node` match `S`, then how many siblings
    /// before it (after it, `from_end`) do. `S` is matched by the same
    /// `Frame::TryAlternatives` `:is()` uses, so `of S` nested inside `of S`
    /// costs heap, not native stack, the way Lexbor's own nested state for it
    /// (`lxb_selectors_state_after_nth_child`) does. It used to be a native
    /// call from here into a fresh `run` per sibling, and 300 levels of it
    /// crashed a 128 KiB Fiber with `SystemStackError`.
    NthOf {
        anb: Nth<'p>,
        from_end: bool,
        lists: Lists<'p>,
        rest: Option<Selector<'p>>,
    },
}

fn check_simple<'p>(
    sel: Selector<'p>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<SimpleCheck<'p>, MatchFailure> {
    Ok(match sel.simple() {
        // `*` matches an ELEMENT, never a text/comment/doctype/PI node -
        // found by the same randomized differential test as
        // `counts_toward_child_position`: `:has(*)` used `*` against every
        // node a `:has()` Descendant search visits, and an unconditional
        // `true` here made a `<p>` with only text content wrongly "have"
        // that text node as a `*`-matching descendant.
        Simple::Universal => SimpleCheck::Result(node.element().is_some()),
        Simple::Type => SimpleCheck::Result(name_eq(node, sel.name())),
        Simple::Id => {
            let ci = document_is_quirks(node);
            SimpleCheck::Result(get_attr(node, b"id").is_some_and(|v| eq_bytes(v, sel.name(), ci)))
        }
        Simple::Class => {
            let ci = document_is_quirks(node);
            let has =
                get_attr(node, b"class").is_some_and(|c| has_whitespace_token(c, sel.name(), ci));
            SimpleCheck::Result(has)
        }
        Simple::Attribute(at) => {
            // §B-4: explicit `i` -> case-insensitive; explicit `s` -> forced
            // case-sensitive; no modifier at all -> the HTML table decides
            // (`attribute_matches`'s `None` case). `Attribute::explicit_sensitive`
            // is what makes the third case distinguishable from the second
            // (see its doc in `css_parser.rs`).
            let explicit_ci = if at.case_insensitive {
                Some(true)
            } else if at.explicit_sensitive {
                Some(false)
            } else {
                None
            };
            SimpleCheck::Result(attribute_matches(
                node,
                sel.name(),
                at.op,
                at.value,
                explicit_ci,
            ))
        }
        Simple::PseudoClass(pc) => SimpleCheck::Result(plain_pseudo_matches(pc, node, budget)?),
        Simple::PseudoClassFunction(FunctionArg::Nth {
            from_end,
            of_type,
            anb,
        }) => match anb.and_then(|a| a.of_list.map(|l| (a, l))) {
            Some((anb, lists)) => SimpleCheck::NthOf {
                anb,
                from_end,
                lists,
                rest: None,
            },
            None => SimpleCheck::Result(nth_matches(node, from_end, of_type, anb, budget)?),
        },
        Simple::PseudoClassFunction(FunctionArg::Selectors {
            pseudo: ListPseudo::Has,
            lists,
        }) => SimpleCheck::Has {
            lists,
            // `check_from` (not this function) knows the compound's chain
            // and fills the real value in.
            rest: None,
        },
        Simple::PseudoClassFunction(FunctionArg::Selectors { pseudo, lists }) => {
            SimpleCheck::Defer {
                negate: pseudo == ListPseudo::Not,
                lists,
                // `check_from` (not this function) knows the compound's
                // chain and fills the real value in.
                rest: None,
            }
        }
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

/// Check `sel`, then every `Close`-linked simple selector after it, in
/// order, eagerly - an ordinary iterative loop (bounded by how many simple
/// selectors one compound chains, e.g. `.a.b.c...`; a large count costs
/// time, never native stack, same as everything else in this module) -
/// until either one fails, they all pass, or a `:is`/`:where`/`:not` is hit,
/// which defers to the explicit stack; `rest` then carries whatever
/// `Close`-linked selectors remain, to run back through THIS function once
/// the deferred verdict is known (see `Cont::CompoundRest`).
fn check_from<'p>(
    mut sel: Selector<'p>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<SimpleCheck<'p>, MatchFailure> {
    loop {
        let next = sel.next().filter(|n| n.combinator() == Combinator::Close);
        match check_simple(sel, node, budget)? {
            SimpleCheck::Result(false) => return Ok(SimpleCheck::Result(false)),
            SimpleCheck::Defer { negate, lists, .. } => {
                return Ok(SimpleCheck::Defer {
                    negate,
                    lists,
                    rest: next,
                });
            }
            SimpleCheck::Has { lists, .. } => {
                return Ok(SimpleCheck::Has { lists, rest: next });
            }
            SimpleCheck::NthOf {
                anb,
                from_end,
                lists,
                ..
            } => {
                return Ok(SimpleCheck::NthOf {
                    anb,
                    from_end,
                    lists,
                    rest: next,
                });
            }
            SimpleCheck::Result(true) => match next {
                Some(n) => sel = n,
                None => return Ok(SimpleCheck::Result(true)),
            },
        }
    }
}

/// Does `compound` match `node`, in full?
fn check_compound<'p>(
    compound: Compound<'p>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<SimpleCheck<'p>, MatchFailure> {
    check_from(compound.first, node, budget)
}

/* ------------------------------------------------------------------ *
 * the explicit work stack - THE point of this module                  *
 * ------------------------------------------------------------------ */

/// A parked continuation: its slot in [`Query`]'s slab.
#[derive(Clone, Copy)]
struct ContId(u32);

enum Frame<'p, 'doc> {
    /// Check `chain[idx]` against `node`; on success, continue leftward
    /// (`idx - 1`) per `chain[idx].comb`, or - if `idx == 0` - report success
    /// to `k`.
    EvalCompound {
        chain: Chain,
        idx: usize,
        node: HtmlNode<'doc>,
        k: Cont<'p, 'doc>,
    },
    /// Try the alternatives of `lists`, in order (`:is`/`:where` = OR,
    /// `negate` flips it for `:not`), against `node`.
    TryAlternatives {
        lists: Lists<'p>,
        node: HtmlNode<'doc>,
        negate: bool,
        k: Cont<'p, 'doc>,
    },
    /// A sub-computation finished with `bool`; propagate it to `k`.
    Deliver(bool, Cont<'p, 'doc>),
    /// `:has()`'s forward search: pull the next candidate from `cursor` and
    /// check `chain[idx]`'s own compound against it, or - once `cursor` is
    /// exhausted - report the whole search as false to `k`. See
    /// [`HasCursor`]'s doc.
    HasStep {
        chain: Chain,
        idx: usize,
        cursor: HasCursor<'doc>,
        k: Cont<'p, 'doc>,
    },
    /// `of S`'s sibling count (§D-1): `pos` is the rank so far; test
    /// `sibling` against `S`, or - once there is none - deliver whether `pos`
    /// satisfies `anb`.
    NthOfStep {
        anb: Nth<'p>,
        from_end: bool,
        lists: Lists<'p>,
        sibling: Option<HtmlNode<'doc>>,
        pos: u64,
        k: Cont<'p, 'doc>,
    },
}

/// What to do with a sub-computation's `bool`. A continuation that waits on
/// another holds it as a [`ContId`] into [`Query`]'s slab rather than a
/// `Box`, so parking one can fail cleanly ([`MatchFailure::Oom`]) instead of
/// aborting.
enum Cont<'p, 'doc> {
    Root,
    /// A deferred list-pseudo inside `chain[idx]`'s compound just resolved;
    /// on failure the whole compound fails (AND semantics), on success check
    /// `rest` - the compound's remaining `Close`-linked simple selectors, if
    /// any - via [`check_from`] again (which may itself defer again, e.g.
    /// `a:not(x):is(y)`), and only once the WHOLE compound is settled, finish
    /// it exactly as a plain compound match would (`advance`).
    CompoundRest {
        rest: Option<Selector<'p>>,
        chain: Chain,
        idx: usize,
        node: HtmlNode<'doc>,
        k: ContId,
    },
    /// Retry `chain[idx]` at the next ancestor of `from` on failure; forward
    /// success as-is.
    AncestorRetry {
        chain: Chain,
        idx: usize,
        from: HtmlNode<'doc>,
        k: ContId,
    },
    /// As `AncestorRetry`, over preceding sibling elements (`~`).
    SiblingRetry {
        chain: Chain,
        idx: usize,
        from: HtmlNode<'doc>,
        k: ContId,
    },
    /// One alternative of an `:is`/`:where`/`:not` list just resolved;
    /// stop (match found, or - for `:not` - disproved) or try the next.
    AlternativeRetry {
        lists: Lists<'p>,
        node: HtmlNode<'doc>,
        negate: bool,
        k: ContId,
    },
    /// A `:has()` search candidate for `chain[idx]` just had its OWN compound
    /// checked ([`single_compound_frame`]); on success, either the `:has()`
    /// alternative succeeds (`idx` was the chain's last compound) or the
    /// search descends to `chain[idx + 1]` FROM this candidate, with
    /// [`Cont::HasBacktrack`] resuming `cursor` here if that fails; on
    /// failure, resume `cursor` for the next candidate at this SAME level.
    HasCandidateChecked {
        chain: Chain,
        idx: usize,
        cursor: HasCursor<'doc>,
        candidate: HtmlNode<'doc>,
        k: ContId,
    },
    /// The descent into `chain[idx + 1]` just concluded; on success the whole
    /// `:has()` alternative succeeds, on failure resume `cursor` - the OUTER
    /// level's remaining candidates for `chain[idx]`.
    HasBacktrack {
        chain: Chain,
        idx: usize,
        cursor: HasCursor<'doc>,
        k: ContId,
    },
    /// One `:has()` alternative's search (`:has(a, b)` = OR) just concluded;
    /// on failure, try the next alternative ([`Query::try_has_alternative`]).
    HasAlternativeRetry {
        lists: Lists<'p>,
        node: HtmlNode<'doc>,
        k: ContId,
    },
    /// `of S`: whether the candidate `node` itself matches `S` just resolved.
    /// It must (§D-1), or the pseudo-class is false; if it does, start
    /// counting its siblings at rank 1.
    NthOfSelf {
        anb: Nth<'p>,
        from_end: bool,
        lists: Lists<'p>,
        node: HtmlNode<'doc>,
        k: ContId,
    },
    /// `of S`: whether `sibling` matches `S` just resolved; count it, and
    /// move on to the next sibling.
    NthOfSibling {
        anb: Nth<'p>,
        from_end: bool,
        lists: Lists<'p>,
        sibling: HtmlNode<'doc>,
        pos: u64,
        k: ContId,
    },
}

/// A continuation slot: in use, or free and linking to the next free one.
enum Slot<'p, 'doc> {
    Used(Cont<'p, 'doc>),
    Free(Option<u32>),
}

/// A [`Frame::EvalCompound`] checking ONLY `chain[idx]`'s own simple
/// selectors (never anything chained after it) against `node`, delivering
/// into `k` - how [`Frame::HasStep`] checks one `:has()` chain compound
/// against one candidate, through the same simple-selector and nested
/// list-pseudo machinery every other compound check uses.
///
/// The sub-chain `chain.only(idx)` is length 1, so `advance` settles it at
/// `idx == 0` without ever reading its combinator - and it follows nothing
/// the ORIGINAL selector chained after this compound (for
/// `:has(section > p.x)`, checking "section" must not become "is `node` a
/// `p.x` whose parent is `section`"). It points into the compiled table, so
/// it costs no allocation.
fn single_compound_frame<'p, 'doc>(
    chain: Chain,
    idx: usize,
    node: HtmlNode<'doc>,
    k: Cont<'p, 'doc>,
) -> Frame<'p, 'doc> {
    Frame::EvalCompound {
        chain: chain.only(idx),
        idx: 0,
        node,
        k,
    }
}

/// One top-level call's matching state: the compiled selector, the work
/// [`Budget`], and the work stack and continuation slab [`Query::run`]
/// uses - both kept (cleared, capacity retained) from one candidate to the
/// next, so a walk over many candidates allocates for the deepest match it
/// needed, not once per candidate. Lexbor does the same with its entry /
/// nested-state object pool, reset between candidates.
///
/// Every continuation is resumed exactly once, so a resumed slot goes back
/// on the free list and the slab holds only the live ones - the property
/// `Box` had, without an allocation that aborts on failure.
struct Query<'c, 'p, 'doc> {
    compiled: &'c Compiled<'p>,
    budget: Budget,
    stack: Vec<Frame<'p, 'doc>>,
    conts: Vec<Slot<'p, 'doc>>,
    free: Option<u32>,
}

impl<'c, 'p, 'doc> Query<'c, 'p, 'doc> {
    fn new(compiled: &'c Compiled<'p>, limit: u64) -> Self {
        Query {
            compiled,
            budget: Budget {
                spent: std::cell::Cell::new(0),
                limit,
            },
            stack: Vec::new(),
            conts: Vec::new(),
            free: None,
        }
    }

    fn push(&mut self, frame: Frame<'p, 'doc>) -> Result<(), MatchFailure> {
        self.stack.falloc_push(frame).or_oom()
    }

    /// Park `k` in the slab, for a continuation that waits on it.
    fn park(&mut self, k: Cont<'p, 'doc>) -> Result<ContId, MatchFailure> {
        if let Some(i) = self.free {
            let slot = self
                .conts
                .get_mut(i as usize)
                .ok_or(MatchFailure::Unsupported)?;
            return match core::mem::replace(slot, Slot::Used(k)) {
                Slot::Free(next) => {
                    self.free = next;
                    Ok(ContId(i))
                }
                // A used slot on the free list is a broken invariant; the
                // query ends here rather than answer from a clobbered state.
                Slot::Used(_) => Err(MatchFailure::Unsupported),
            };
        }
        let i = u32::try_from(self.conts.len()).map_err(|_| MatchFailure::Oom)?;
        self.conts.falloc_push(Slot::Used(k)).or_oom()?;
        Ok(ContId(i))
    }

    /// Take the parked continuation back, freeing its slot.
    fn resume(&mut self, id: ContId) -> Result<Cont<'p, 'doc>, MatchFailure> {
        let free = self.free;
        let slot = self
            .conts
            .get_mut(id.0 as usize)
            .ok_or(MatchFailure::Unsupported)?;
        match core::mem::replace(slot, Slot::Free(free)) {
            Slot::Used(k) => {
                self.free = Some(id.0);
                Ok(k)
            }
            Slot::Free(next) => {
                // Resumed twice: a broken invariant, reported, not answered.
                *slot = Slot::Free(next);
                Err(MatchFailure::Unsupported)
            }
        }
    }

    /// Hand `result` to the parked continuation `k`.
    fn deliver(&mut self, result: bool, k: ContId) -> Result<(), MatchFailure> {
        let k = self.resume(k)?;
        self.push(Frame::Deliver(result, k))
    }

    /// Does `node` match any top-level alternative?
    fn matches_top(&mut self, node: HtmlNode<'doc>) -> Result<bool, MatchFailure> {
        for i in 0..self.compiled.top.len() {
            let chain = self.compiled.top[i];
            if chain.len != 0
                && self.run(Frame::EvalCompound {
                    chain,
                    idx: chain.len() - 1,
                    node,
                    k: Cont::Root,
                })?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// After `chain[idx]` is known to match (or not) at `node`, do what an
    /// ordinary (non-deferred) hit would: on failure, fail the whole chain;
    /// on success, continue left (or report success, at `idx == 0`).
    fn advance(
        &mut self,
        chain: Chain,
        idx: usize,
        node: HtmlNode<'doc>,
        k: Cont<'p, 'doc>,
        matched: bool,
    ) -> Result<(), MatchFailure> {
        if !matched {
            return self.push(Frame::Deliver(false, k));
        }
        if idx == 0 {
            return self.push(Frame::Deliver(true, k));
        }
        let next_idx = idx - 1;
        match self.compiled.compound(chain, idx).comb {
            Combinator::Close | Combinator::Child => match parent_element(node) {
                Some(p) => self.push(Frame::EvalCompound {
                    chain,
                    idx: next_idx,
                    node: p,
                    k,
                }),
                None => self.push(Frame::Deliver(false, k)),
            },
            Combinator::Descendant => match parent_element(node) {
                Some(p) => {
                    let k = self.park(k)?;
                    self.push(Frame::EvalCompound {
                        chain,
                        idx: next_idx,
                        node: p,
                        k: Cont::AncestorRetry {
                            chain,
                            idx: next_idx,
                            from: p,
                            k,
                        },
                    })
                }
                None => self.push(Frame::Deliver(false, k)),
            },
            Combinator::NextSibling => match prev_sibling_element(node) {
                Some(s) => self.push(Frame::EvalCompound {
                    chain,
                    idx: next_idx,
                    node: s,
                    k,
                }),
                None => self.push(Frame::Deliver(false, k)),
            },
            Combinator::SubsequentSibling => match prev_sibling_element(node) {
                Some(s) => {
                    let k = self.park(k)?;
                    self.push(Frame::EvalCompound {
                        chain,
                        idx: next_idx,
                        node: s,
                        k: Cont::SiblingRetry {
                            chain,
                            idx: next_idx,
                            from: s,
                            k,
                        },
                    })
                }
                None => self.push(Frame::Deliver(false, k)),
            },
            // The column combinator `||`: `compile` refuses it before
            // matching starts; this is the belt to that brace.
            Combinator::Other => Err(MatchFailure::Unsupported),
        }
    }

    /// The first non-empty alternative left in `lists`, or `None`.
    fn next_alternative(&self, lists: &mut Lists<'p>) -> Result<Option<Chain>, MatchFailure> {
        for list in lists.by_ref() {
            let chain = self.compiled.nested_chain(list)?;
            if chain.len != 0 {
                return Ok(Some(chain));
            }
        }
        Ok(None)
    }

    fn try_alternative(
        &mut self,
        mut lists: Lists<'p>,
        node: HtmlNode<'doc>,
        negate: bool,
        k: Cont<'p, 'doc>,
    ) -> Result<(), MatchFailure> {
        let Some(chain) = self.next_alternative(&mut lists)? else {
            // Out of alternatives: :is/:where found none (false); :not found
            // none that matched, so it holds (true).
            return self.push(Frame::Deliver(negate, k));
        };
        let k = self.park(k)?;
        self.push(Frame::EvalCompound {
            chain,
            idx: chain.len() - 1,
            node,
            k: Cont::AlternativeRetry {
                lists,
                node,
                negate,
                k,
            },
        })
    }

    /// Try `:has()`'s comma-separated alternatives (`:has(a, b)` = OR) in
    /// order, each a FORWARD SEARCH from `node` - unlike
    /// [`Query::try_alternative`]'s, which match AT `node` itself.
    fn try_has_alternative(
        &mut self,
        mut lists: Lists<'p>,
        node: HtmlNode<'doc>,
        k: Cont<'p, 'doc>,
    ) -> Result<(), MatchFailure> {
        let Some(chain) = self.next_alternative(&mut lists)? else {
            // Out of alternatives: :has() found nothing.
            return self.push(Frame::Deliver(false, k));
        };
        let cursor = HasCursor::start(self.compiled.compound(chain, 0).comb, node)?;
        let k = self.park(k)?;
        self.push(Frame::HasStep {
            chain,
            idx: 0,
            cursor,
            k: Cont::HasAlternativeRetry { lists, node, k },
        })
    }

    /// Act on `chain[idx]`'s compound check at `node`: a settled verdict goes
    /// to [`Query::advance`]; a deferred list-pseudo is pushed with a
    /// `Cont::CompoundRest` that resumes the compound's remaining simple
    /// selectors once it resolves. The one place both a fresh compound
    /// (`Frame::EvalCompound`) and a resumed one (`Cont::CompoundRest`)
    /// dispatch, so a new deferred kind is added once.
    fn settle(
        &mut self,
        check: SimpleCheck<'p>,
        chain: Chain,
        idx: usize,
        node: HtmlNode<'doc>,
        k: Cont<'p, 'doc>,
    ) -> Result<(), MatchFailure> {
        let (rest, deferred) = match check {
            SimpleCheck::Result(m) => return self.advance(chain, idx, node, k, m),
            SimpleCheck::Defer {
                negate,
                lists,
                rest,
            } => (rest, Deferred::Alternatives { negate, lists }),
            SimpleCheck::Has { lists, rest } => (rest, Deferred::Has { lists }),
            SimpleCheck::NthOf {
                anb,
                from_end,
                lists,
                rest,
            } => (
                rest,
                Deferred::NthOf {
                    anb,
                    from_end,
                    lists,
                },
            ),
        };
        let k = self.park(k)?;
        let resume = Cont::CompoundRest {
            rest,
            chain,
            idx,
            node,
            k,
        };
        match deferred {
            Deferred::Alternatives { negate, lists } => self.push(Frame::TryAlternatives {
                lists,
                node,
                negate,
                k: resume,
            }),
            Deferred::Has { lists } => self.try_has_alternative(lists, node, resume),
            Deferred::NthOf {
                anb,
                from_end,
                lists,
            } => {
                let k = self.park(resume)?;
                self.push(Frame::TryAlternatives {
                    lists,
                    node,
                    negate: false,
                    k: Cont::NthOfSelf {
                        anb,
                        from_end,
                        lists,
                        node,
                        k,
                    },
                })
            }
        }
    }

    /// Run the machine from `first` to completion. The loop is the WHOLE
    /// control flow - no Rust-level recursion anywhere here, however deeply
    /// the selector nests. Charges the budget once per frame popped, so a
    /// `:is()` alternative, a long ancestor climb or a `:has()` candidate is
    /// bounded by the same budget.
    fn run(&mut self, first: Frame<'p, 'doc>) -> Result<bool, MatchFailure> {
        // Nothing survives from one run to the next; an earlier run that
        // failed ended the query, so this only drops what that left behind.
        self.stack.clear();
        self.conts.clear();
        self.free = None;
        self.push(first)?;
        let mut answer = false;
        while let Some(frame) = self.stack.pop() {
            self.budget.charge()?;
            match frame {
                Frame::EvalCompound {
                    chain,
                    idx,
                    node,
                    k,
                } => {
                    let check =
                        check_compound(self.compiled.compound(chain, idx), node, &self.budget)?;
                    self.settle(check, chain, idx, node, k)?;
                }
                Frame::NthOfStep {
                    anb,
                    from_end,
                    lists,
                    sibling,
                    pos,
                    k,
                } => match sibling {
                    None => self.push(Frame::Deliver(
                        anb_matches(anb, i64::try_from(pos).unwrap_or(i64::MAX)),
                        k,
                    ))?,
                    Some(sibling) => {
                        let k = self.park(k)?;
                        self.push(Frame::TryAlternatives {
                            lists,
                            node: sibling,
                            negate: false,
                            k: Cont::NthOfSibling {
                                anb,
                                from_end,
                                lists,
                                sibling,
                                pos,
                                k,
                            },
                        })?;
                    }
                },
                Frame::TryAlternatives {
                    lists,
                    node,
                    negate,
                    k,
                } => self.try_alternative(lists, node, negate, k)?,
                Frame::HasStep {
                    chain,
                    idx,
                    mut cursor,
                    k,
                } => match cursor.next() {
                    None => self.push(Frame::Deliver(false, k))?,
                    Some(candidate) => {
                        let k = self.park(k)?;
                        self.push(single_compound_frame(
                            chain,
                            idx,
                            candidate,
                            Cont::HasCandidateChecked {
                                chain,
                                idx,
                                cursor,
                                candidate,
                                k,
                            },
                        ))?;
                    }
                },
                Frame::Deliver(result, cont) => self.deliver_to(result, cont, &mut answer)?,
            }
        }
        Ok(answer)
    }

    /// `run`'s `Frame::Deliver`: hand `result` to `cont`.
    fn deliver_to(
        &mut self,
        result: bool,
        cont: Cont<'p, 'doc>,
        answer: &mut bool,
    ) -> Result<(), MatchFailure> {
        match cont {
            Cont::Root => {
                *answer = result;
                Ok(())
            }
            Cont::CompoundRest {
                rest,
                chain,
                idx,
                node,
                k,
            } => {
                let k = self.resume(k)?;
                match (result, rest) {
                    (false, _) => self.advance(chain, idx, node, k, false),
                    (true, None) => self.advance(chain, idx, node, k, true),
                    (true, Some(next)) => {
                        let check = check_from(next, node, &self.budget)?;
                        self.settle(check, chain, idx, node, k)
                    }
                }
            }
            Cont::AncestorRetry {
                chain,
                idx,
                from,
                k,
            } => {
                if result {
                    return self.deliver(true, k);
                }
                match parent_element(from) {
                    Some(p) => self.push(Frame::EvalCompound {
                        chain,
                        idx,
                        node: p,
                        k: Cont::AncestorRetry {
                            chain,
                            idx,
                            from: p,
                            k,
                        },
                    }),
                    None => self.deliver(false, k),
                }
            }
            Cont::SiblingRetry {
                chain,
                idx,
                from,
                k,
            } => {
                if result {
                    return self.deliver(true, k);
                }
                match prev_sibling_element(from) {
                    Some(s) => self.push(Frame::EvalCompound {
                        chain,
                        idx,
                        node: s,
                        k: Cont::SiblingRetry {
                            chain,
                            idx,
                            from: s,
                            k,
                        },
                    }),
                    None => self.deliver(false, k),
                }
            }
            Cont::AlternativeRetry {
                lists,
                node,
                negate,
                k,
            } => {
                if result {
                    // This alternative matched: :is/:where succeeds; :not is
                    // disproved. Either way, the rest need not be tried.
                    return self.deliver(!negate, k);
                }
                let k = self.resume(k)?;
                self.try_alternative(lists, node, negate, k)
            }
            Cont::HasCandidateChecked {
                chain,
                idx,
                cursor,
                candidate,
                k,
            } => {
                if !result {
                    // This candidate's own compound didn't match; try the
                    // next one at the SAME level.
                    let k = self.resume(k)?;
                    return self.push(Frame::HasStep {
                        chain,
                        idx,
                        cursor,
                        k,
                    });
                }
                if idx + 1 == chain.len() {
                    // This candidate satisfied the WHOLE :has() chain.
                    return self.deliver(true, k);
                }
                // Descend to chain[idx + 1] FROM this candidate; a failure
                // down there resumes `cursor` here (HasBacktrack).
                let next_cursor =
                    HasCursor::start(self.compiled.compound(chain, idx + 1).comb, candidate)?;
                self.push(Frame::HasStep {
                    chain,
                    idx: idx + 1,
                    cursor: next_cursor,
                    k: Cont::HasBacktrack {
                        chain,
                        idx,
                        cursor,
                        k,
                    },
                })
            }
            Cont::HasBacktrack {
                chain,
                idx,
                cursor,
                k,
            } => {
                if result {
                    return self.deliver(true, k);
                }
                // The deeper search found nothing from THIS candidate; try
                // the next one at THIS level.
                let k = self.resume(k)?;
                self.push(Frame::HasStep {
                    chain,
                    idx,
                    cursor,
                    k,
                })
            }
            Cont::HasAlternativeRetry { lists, node, k } => {
                if result {
                    return self.deliver(true, k);
                }
                let k = self.resume(k)?;
                self.try_has_alternative(lists, node, k)
            }
            Cont::NthOfSelf {
                anb,
                from_end,
                lists,
                node,
                k,
            } => {
                let k = self.resume(k)?;
                if !result {
                    return self.push(Frame::Deliver(false, k));
                }
                self.push(Frame::NthOfStep {
                    anb,
                    from_end,
                    lists,
                    sibling: nth_of_sibling(node, from_end),
                    pos: 1,
                    k,
                })
            }
            Cont::NthOfSibling {
                anb,
                from_end,
                lists,
                sibling,
                pos,
                k,
            } => {
                let k = self.resume(k)?;
                self.push(Frame::NthOfStep {
                    anb,
                    from_end,
                    lists,
                    sibling: nth_of_sibling(sibling, from_end),
                    pos: pos + u64::from(result),
                    k,
                })
            }
        }
    }
}

/// [`Query::settle`]'s deferred kinds, split from their shared `rest`.
enum Deferred<'p> {
    Alternatives {
        negate: bool,
        lists: Lists<'p>,
    },
    Has {
        lists: Lists<'p>,
    },
    NthOf {
        anb: Nth<'p>,
        from_end: bool,
        lists: Lists<'p>,
    },
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
    let compiled = compile(groups)?;
    Query::new(&compiled, DEFAULT_WORK_BUDGET).matches_top(element.node())
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
    select_all_with_limit(root, groups, DEFAULT_WORK_BUDGET)
}

fn select_all_with_limit<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
    limit: u64,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    let compiled = compile(groups)?;
    let mut query = Query::new(&compiled, limit);
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
    select_all_with_limit(root, groups, limit)
}

/// The first descendant of `root`, in document order, that matches any
/// alternative of `groups` - `root` itself excluded. Stops at the first hit
/// instead of building the whole set, as `Node#at_css` wants (see
/// `lexbor::selectors::first_cb`). One work budget for the whole search.
pub fn select_first<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
    let compiled = compile(groups)?;
    let mut query = Query::new(&compiled, DEFAULT_WORK_BUDGET);
    let mut n = root;
    while let Some(next) = n.preorder_next(root) {
        n = next;
        if n.element().is_some() && query.matches_top(n)? {
            return Ok(Some(n));
        }
    }
    Ok(None)
}
