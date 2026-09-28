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
//! - `:nth-child(of S)` is a single flat pass here (count every earlier
//!   S-matching sibling), not Lexbor's streaming nested-matcher loop
//!   (§D-1) - the semantics documented there are equivalent; the C control
//!   flow is not.
//!
//! Still open (tracked in the plan, not silent gaps): `::pseudo-elements`,
//! `:lexbor-contains()` (decided not to reimplement), `:current()` (deferred,
//! not ruled out - `notes/css_selectors_crate_migration_plan.ja.md`), and
//! `falloc` (this file still uses the ordinary allocator - `Box`/`Vec` - like
//! the earlier spike did; see the one `#[allow]`ed `boxed()` helper). The
//! selector-nesting cap question is closed, not open - see the next section -
//! and so is the work budget: see [`Budget`].
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
//! enforced by `collect_compounds`) bounded it safely. That bound was real
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

use std::rc::Rc;

use crate::lexbor::adapter::html::{HtmlElement, HtmlNode, NodeType, NsId};
use crate::lexbor::css_parser::{
    AttrMatch, Combinator, FunctionArg, ListPseudo, Lists, PseudoClass, Selector, Simple,
};
use crate::limits::NODE_SET_MAX;

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
}

impl Budget {
    fn new() -> Self {
        Budget {
            spent: std::cell::Cell::new(0),
            limit: DEFAULT_WORK_BUDGET,
        }
    }

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

/// `Box::new`, allocator-scoped: `clippy.toml` bans it crate-wide in engine
/// layers (aborts on OOM; `falloc::try_box` is the real one), and this
/// module keeps continuations on the ordinary heap rather than through
/// `falloc` for now (see the module doc). One named function keeps the
/// exception to one documented spot instead of scattering `#[allow]`s.
#[allow(
    clippy::disallowed_methods,
    reason = "not yet falloc-backed; see the module doc"
)]
fn boxed<T>(v: T) -> Box<T> {
    Box::new(v)
}

/* ------------------------------------------------------------------ *
 * reading a chain into compounds (iterative; mirrors                 *
 * `css::lower::Compounds`, not shared with it - that module is        *
 * XPath-lowering-specific, this one is self-contained)                *
 * ------------------------------------------------------------------ */

/// A compound (a `Close`-linked run of simple selectors) plus the combinator
/// that attaches it to the compound BEFORE it (to its left - more
/// ancestor/earlier-sibling-ward) in the written selector.
#[derive(Clone, Copy)]
struct Compound<'p> {
    first: Selector<'p>,
    comb: Combinator,
}

/// A whole compound chain, shared rather than owned: `Frame`/`Cont` used to
/// carry `Vec<Compound<'p>>` by value, so `advance`'s `Descendant`/
/// `SubsequentSibling` retries - which keep matching the SAME chain against a
/// new ancestor/sibling on failure - paid a full `Vec` clone (allocation +
/// copy) on every step of the climb. `Compound` is `Copy` and small, so the
/// clone was never about the DATA; it was about `Vec` not being shareable.
/// `Rc<[Compound]>` fixes that: cloning it is a refcount bump, `[idx]`/`.len()`
/// still work via `Deref<Target = [Compound]>`, and building one is done ONCE
/// per selector (`collect_compounds`) rather than once per retry.
type Chain<'p> = Rc<[Compound<'p>]>;

/// Split a chain into compounds, left to right (the order `css_parser` links
/// them in). `None` for an empty chain or one over `MAX_COMPOUNDS`.
///
/// Collected into an `Rc<[Compound]>` via `FromIterator` (one exact-sized
/// allocation - `Vec`'s iterator is `ExactSizeIterator`) rather than
/// `Vec::into()`, which goes through `Vec::into_boxed_slice`'s
/// allocate-then-shrink and is exactly the pattern `clippy.toml`'s
/// `into_boxed_slice` ban calls out (its `.into()` spelling "cannot be named"
/// by that lint, but the reason still applies here).
fn collect_compounds(first: Option<Selector<'_>>) -> Option<Chain<'_>> {
    let mut v = Vec::new();
    let mut cur = first;
    while let Some(start) = cur {
        let comb = start.combinator();
        let mut last = start;
        while let Some(nxt) = last.next().filter(|n| n.combinator() == Combinator::Close) {
            last = nxt;
        }
        v.push(Compound { first: start, comb });
        if v.len() > MAX_COMPOUNDS {
            return None;
        }
        cur = last.next();
    }
    if v.is_empty() {
        None
    } else {
        Some(v.into_iter().collect())
    }
}

/// A comma-separated selector list, each alternative pre-split into a
/// [`Chain`] ONCE - the per-QUERY half of the hoist `select_all`/
/// `select_first`'s tree walk and `sibling_position_of`'s per-sibling loop
/// both need: without it, [`list_matches`] (via [`matches_one_compound_chain`]
/// -> `collect_compounds`) re-walked the parsed selector AST and allocated a
/// fresh [`Chain`] for every CANDIDATE node, even though the chain is fixed
/// for the whole query. A direct Ruby-free timing comparison against
/// Lexbor's own (arena-parsed-once) matcher measured this as a consistent
/// ~6x per-candidate slowdown on both `css` and `at_css` before this fix
/// (see CLAUDE.md's CSS performance note); handing a [`Chain`] to `run`'s
/// initial frame is now `Rc::clone`, not a rebuild.
struct CompiledList<'p>(Vec<Chain<'p>>);

/// Build a [`CompiledList`] once, before the per-candidate loop starts.
fn compile_list(list: Lists<'_>) -> CompiledList<'_> {
    let mut v = Vec::new();
    for l in list {
        if let Some(chain) = collect_compounds(l.first()) {
            v.push(chain);
        }
    }
    CompiledList(v)
}

/// As [`list_matches`], over a [`compile_list`]d [`Lists`] - the per-candidate
/// half of the hoist (see [`CompiledList`]'s doc). Semantically identical to
/// `list_matches(list, node, budget)` for the `Lists` `compiled` was built
/// from: a plain OR over its alternatives.
fn list_matches_compiled(
    compiled: &CompiledList<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    for chain in &compiled.0 {
        let idx = chain.len() - 1;
        let matched = run(
            vec![Frame::EvalCompound {
                chain: Rc::clone(chain),
                idx,
                node,
                k: Cont::Root,
            }],
            budget,
        )?;
        if matched {
            return Ok(true);
        }
    }
    Ok(false)
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

/// §D-1's `of S`: `node`'s 1-based position counting only elements matching
/// `list` among its (`from_end`-directed) siblings, or `None` if `node`
/// itself does not match `list`. A flat pass, not Lexbor's streaming nested
/// matcher (module doc) - the position it computes is the same number.
fn sibling_position_of(
    node: HtmlNode<'_>,
    from_end: bool,
    list: Lists<'_>,
    budget: &Budget,
) -> Result<Option<u64>, MatchFailure> {
    // Hoisted once for the whole sibling scan below - see `CompiledList`'s
    // doc: without it, every sibling visited re-collected `list`'s compounds
    // from scratch, the same per-candidate cost `select_all`/`select_first`
    // had.
    let compiled = compile_list(list);
    if !list_matches_compiled(&compiled, node, budget)? {
        return Ok(None);
    }
    let mut pos: u64 = 1;
    let mut cur = if from_end {
        next_sibling_element(node)
    } else {
        prev_sibling_element(node)
    };
    while let Some(n) = cur {
        if list_matches_compiled(&compiled, n, budget)? {
            pos += 1;
        }
        cur = if from_end {
            next_sibling_element(n)
        } else {
            prev_sibling_element(n)
        };
    }
    Ok(Some(pos))
}

/// Does ANY alternative of `list` match `node`? Used by `:nth-child(of S)`
/// and (via [`matches_one_compound`]) nowhere else - kept separate from the
/// `Frame`/`Cont` machine because `of S` is evaluated OUTSIDE the compound
/// being matched (against arbitrary siblings, not just `node` itself), so it
/// cannot defer through the normal per-node `Cont` chain.
fn list_matches(
    list: Lists<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    for l in list {
        if let Some(first) = l.first() {
            if matches_one_compound_chain(first, node, budget)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Does `node` satisfy the WHOLE (possibly multi-compound) chain starting at
/// `first`, matched right-to-left as an ordinary (non-`:has`) selector would
/// be? This is [`matches`] without the `HtmlElement` wrapper, reused by
/// [`list_matches`] (so `of S`, `select_all`/`select_first`, and
/// `matches_any` all go through the exact same `:is`/`:where`/`:not`/`:has`-
/// nesting-safe machinery `matches` does, rather than a second, weaker
/// implementation).
fn matches_one_compound_chain(
    first: Selector<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    let Some(compounds) = collect_compounds(Some(first)) else {
        return Ok(false);
    };
    let idx = compounds.len() - 1;
    run(
        vec![Frame::EvalCompound {
            chain: compounds,
            idx,
            node,
            k: Cont::Root,
        }],
        budget,
    )
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

/// §D-1's `of S`: `pos` is `node`'s 1-based rank among `S`-matching siblings.
fn nth_of_s_matches(
    node: HtmlNode<'_>,
    from_end: bool,
    list: Lists<'_>,
    anb: Option<crate::lexbor::css_parser::Nth<'_>>,
    budget: &Budget,
) -> Result<bool, MatchFailure> {
    let Some(anb) = anb else {
        return Ok(false);
    };
    let Some(pos) = sibling_position_of(node, from_end, list, budget)? else {
        return Ok(false);
    };
    Ok(anb_matches(anb, pos as i64))
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
    /// `collect_compounds`'s own doc: a compound boundary is always a real
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
        }) => {
            let matched = match anb.and_then(|a| a.of_list.map(|l| (a, l))) {
                Some((a, list)) => nth_of_s_matches(node, from_end, list, Some(a), budget)?,
                None => nth_matches(node, from_end, of_type, anb, budget)?,
            };
            SimpleCheck::Result(matched)
        }
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

enum Frame<'p, 'doc> {
    /// Check `chain[idx]` against `node`; on success, continue leftward
    /// (`idx - 1`) per `chain[idx].comb`, or - if `idx == 0` - report success
    /// to `k`.
    EvalCompound {
        chain: Chain<'p>,
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
        chain: Chain<'p>,
        idx: usize,
        cursor: HasCursor<'doc>,
        k: Cont<'p, 'doc>,
    },
}

enum Cont<'p, 'doc> {
    Root,
    /// A deferred `:is`/`:where`/`:not` inside `chain[idx]`'s compound just
    /// resolved to a boolean; on failure the whole compound fails (AND
    /// semantics), on success check `rest` - the compound's remaining
    /// `Close`-linked simple selectors, if any - via [`check_from`] again
    /// (which may itself defer again, e.g. `a:not(x):is(y)`), and only once
    /// the WHOLE compound is settled, finish it exactly as a plain compound
    /// match would (the idx==0 / combinator dispatch `advance` does).
    CompoundRest {
        rest: Option<Selector<'p>>,
        chain: Chain<'p>,
        idx: usize,
        node: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// Retry `chain[idx]` at the next ancestor of `from` on failure; forward
    /// success as-is.
    AncestorRetry {
        chain: Chain<'p>,
        idx: usize,
        from: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// As `AncestorRetry`, over preceding sibling elements (`~`).
    SiblingRetry {
        chain: Chain<'p>,
        idx: usize,
        from: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// One alternative of an `:is`/`:where`/`:not` list just resolved to
    /// `bool`; decide whether to stop (match found, or - for `:not` -
    /// disproved) or try the next alternative.
    AlternativeRetry {
        lists: Lists<'p>,
        node: HtmlNode<'doc>,
        negate: bool,
        k: Box<Cont<'p, 'doc>>,
    },
    /// A `:has()` search candidate for `chain[idx]` just had its OWN compound
    /// checked (via [`single_compound_frame`]); on success, either the
    /// `:has()` succeeds (`idx` was the chain's last compound) or the search
    /// descends to `chain[idx + 1]` FROM this candidate (a fresh
    /// [`Frame::HasStep`], with [`Cont::HasBacktrack`] as ITS `k` so a
    /// failure down there resumes `cursor` here); on failure, resume `cursor`
    /// directly for the next candidate at this SAME level. Never a native
    /// call either way - this is what replaces `has_forward`'s own
    /// compound-by-compound recursion (module doc).
    HasCandidateChecked {
        chain: Chain<'p>,
        idx: usize,
        cursor: HasCursor<'doc>,
        candidate: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// The descent into `chain[idx + 1]` (from a candidate that matched
    /// `chain[idx]`) just concluded; on success the whole `:has()`
    /// alternative succeeds, on failure resume `cursor` - the OUTER level's
    /// remaining candidates for `chain[idx]` - exactly the backtrack
    /// `has_forward`'s own recursion return value used to drive natively.
    HasBacktrack {
        chain: Chain<'p>,
        idx: usize,
        cursor: HasCursor<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// One `:has()` alternative's search (`:has(a, b)` = OR) just concluded;
    /// on failure, try the next alternative ([`try_has_alternative`]) -
    /// `:has()` has no `:not`-style negation at this level, unlike
    /// `AlternativeRetry`.
    HasAlternativeRetry {
        lists: Lists<'p>,
        node: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
}

/// After `chain[idx]` is known to match (or not) at `node`, do what an
/// ordinary (non-deferred) hit would: on failure, fail the whole chain; on
/// success, continue left (or report success, at `idx == 0`).
fn advance<'p, 'doc>(
    stack: &mut Vec<Frame<'p, 'doc>>,
    chain: Chain<'p>,
    idx: usize,
    node: HtmlNode<'doc>,
    k: Cont<'p, 'doc>,
    matched: bool,
) -> Result<(), MatchFailure> {
    if !matched {
        stack.push(Frame::Deliver(false, k));
        return Ok(());
    }
    if idx == 0 {
        stack.push(Frame::Deliver(true, k));
        return Ok(());
    }
    let comb = chain[idx].comb;
    let next_idx = idx - 1;
    match comb {
        Combinator::Close | Combinator::Child => match parent_element(node) {
            Some(p) => stack.push(Frame::EvalCompound {
                chain,
                idx: next_idx,
                node: p,
                k,
            }),
            None => stack.push(Frame::Deliver(false, k)),
        },
        Combinator::Descendant => match parent_element(node) {
            Some(p) => stack.push(Frame::EvalCompound {
                chain: chain.clone(),
                idx: next_idx,
                node: p,
                k: Cont::AncestorRetry {
                    chain,
                    idx: next_idx,
                    from: p,
                    k: boxed(k),
                },
            }),
            None => stack.push(Frame::Deliver(false, k)),
        },
        Combinator::NextSibling => match prev_sibling_element(node) {
            Some(s) => stack.push(Frame::EvalCompound {
                chain,
                idx: next_idx,
                node: s,
                k,
            }),
            None => stack.push(Frame::Deliver(false, k)),
        },
        Combinator::SubsequentSibling => match prev_sibling_element(node) {
            Some(s) => stack.push(Frame::EvalCompound {
                chain: chain.clone(),
                idx: next_idx,
                node: s,
                k: Cont::SiblingRetry {
                    chain,
                    idx: next_idx,
                    from: s,
                    k: boxed(k),
                },
            }),
            None => stack.push(Frame::Deliver(false, k)),
        },
        // The column combinator `||`: see `Combinator::Other`'s handling in
        // `has_forward` (this file's other combinator dispatch) - same
        // reasoning, same error.
        Combinator::Other => return Err(MatchFailure::Unsupported),
    }
    Ok(())
}

fn try_alternative<'p, 'doc>(
    stack: &mut Vec<Frame<'p, 'doc>>,
    mut lists: Lists<'p>,
    node: HtmlNode<'doc>,
    negate: bool,
    k: Cont<'p, 'doc>,
) {
    loop {
        let Some(list) = lists.next() else {
            // Out of alternatives: :is/:where found none (false); :not found
            // none that matched, so it holds (true).
            stack.push(Frame::Deliver(negate, k));
            return;
        };
        let Some(compounds) = collect_compounds(list.first()) else {
            // An empty or over-complex alternative never matches; try the rest.
            continue;
        };
        let idx = compounds.len() - 1;
        stack.push(Frame::EvalCompound {
            chain: compounds,
            idx,
            node,
            k: Cont::AlternativeRetry {
                lists,
                node,
                negate,
                k: boxed(k),
            },
        });
        return;
    }
}

/// A [`Frame::EvalCompound`] checking ONLY `compound`'s own simple selectors
/// (never anything chained after it) against `node`, delivering into `k` -
/// the building block [`Frame::HasStep`]'s handling uses to check one
/// `:has()` chain compound against one candidate, reusing the exact same
/// simple-selector + nested-`:is`/`:where`/`:not`/`:has` resolution machinery
/// every other compound check here already goes through.
///
/// Deliberately NOT wrapping `compound.first` via `collect_compounds`/
/// [`matches_one_compound_chain`]: that would walk PAST this one compound to
/// whatever the ORIGINAL selector chained after it (e.g. for
/// `:has(section > p.x)`, `compound` is "section" alone, but
/// `compound.first.next()` is "p.x" via a `Child` combinator - re-deriving
/// from there would ask "is `node` a `p.x` whose parent is `section`", the
/// wrong question - `Frame::HasStep`'s own `idx` stepping already answers one
/// level up). Wrapping `compound` in a length-1 chain (`comb: Close`, as
/// `idx == 0` never reads it - see `advance`) checks ONLY its own simple
/// selectors.
fn single_compound_frame<'p, 'doc>(
    compound: Compound<'p>,
    node: HtmlNode<'doc>,
    k: Cont<'p, 'doc>,
) -> Frame<'p, 'doc> {
    Frame::EvalCompound {
        chain: Rc::from([Compound {
            first: compound.first,
            comb: Combinator::Close,
        }]),
        idx: 0,
        node,
        k,
    }
}

/// Try `:has()`'s comma-separated alternatives (`:has(a, b)` = OR) in order,
/// each a FORWARD SEARCH from `node` - unlike [`try_alternative`]'s `:is`/
/// `:where`/`:not` alternatives, which match AT `node` itself. Heap-based
/// throughout (module doc): starting a search can only fail on the column
/// combinator (`Combinator::Other`, [`HasCursor::start`]), propagated
/// immediately since nothing about THIS alternative has been pushed onto
/// `stack` yet for it to unwind through.
fn try_has_alternative<'p, 'doc>(
    stack: &mut Vec<Frame<'p, 'doc>>,
    mut lists: Lists<'p>,
    node: HtmlNode<'doc>,
    k: Cont<'p, 'doc>,
) -> Result<(), MatchFailure> {
    loop {
        let Some(list) = lists.next() else {
            // Out of alternatives: :has() found nothing.
            stack.push(Frame::Deliver(false, k));
            return Ok(());
        };
        let Some(chain) = collect_compounds(list.first()) else {
            // An empty or over-complex alternative never matches; try the rest.
            continue;
        };
        let cursor = HasCursor::start(chain[0].comb, node)?;
        stack.push(Frame::HasStep {
            chain,
            idx: 0,
            cursor,
            k: Cont::HasAlternativeRetry {
                lists,
                node,
                k: boxed(k),
            },
        });
        return Ok(());
    }
}

/// Run the machine to completion. `stack` starts with exactly one frame; the
/// loop is the WHOLE control flow - no Rust-level recursion anywhere here,
/// regardless of how deeply the selector nests. Charges `budget` once per
/// frame popped - an ancestor/sibling retry that failed and is trying the
/// next candidate re-enters this loop with a fresh frame, so a `:is()`
/// alternative or a long ancestor climb is bounded by the same budget
/// `:has()`'s own search is, not just by depth counts.
fn run(mut stack: Vec<Frame<'_, '_>>, budget: &Budget) -> Result<bool, MatchFailure> {
    let mut answer = false;
    while let Some(frame) = stack.pop() {
        budget.charge()?;
        match frame {
            Frame::EvalCompound {
                chain,
                idx,
                node,
                k,
            } => match check_compound(chain[idx], node, budget)? {
                SimpleCheck::Result(m) => advance(&mut stack, chain, idx, node, k, m)?,
                SimpleCheck::Defer {
                    negate,
                    lists,
                    rest,
                } => stack.push(Frame::TryAlternatives {
                    lists,
                    node,
                    negate,
                    k: Cont::CompoundRest {
                        rest,
                        chain,
                        idx,
                        node,
                        k: boxed(k),
                    },
                }),
                SimpleCheck::Has { lists, rest } => try_has_alternative(
                    &mut stack,
                    lists,
                    node,
                    Cont::CompoundRest {
                        rest,
                        chain,
                        idx,
                        node,
                        k: boxed(k),
                    },
                )?,
            },
            Frame::TryAlternatives {
                lists,
                node,
                negate,
                k,
            } => try_alternative(&mut stack, lists, node, negate, k),
            Frame::HasStep {
                chain,
                idx,
                mut cursor,
                k,
            } => match cursor.next() {
                None => stack.push(Frame::Deliver(false, k)),
                Some(candidate) => stack.push(single_compound_frame(
                    chain[idx],
                    candidate,
                    Cont::HasCandidateChecked {
                        chain,
                        idx,
                        cursor,
                        candidate,
                        k: boxed(k),
                    },
                )),
            },
            Frame::Deliver(result, cont) => match cont {
                Cont::Root => answer = result,
                Cont::CompoundRest {
                    rest,
                    chain,
                    idx,
                    node,
                    k,
                } => {
                    if !result {
                        advance(&mut stack, chain, idx, node, *k, false)?;
                    } else {
                        match rest {
                            None => advance(&mut stack, chain, idx, node, *k, true)?,
                            Some(next) => match check_from(next, node, budget)? {
                                SimpleCheck::Result(m) => {
                                    advance(&mut stack, chain, idx, node, *k, m)?
                                }
                                SimpleCheck::Defer {
                                    negate,
                                    lists,
                                    rest,
                                } => {
                                    stack.push(Frame::TryAlternatives {
                                        lists,
                                        node,
                                        negate,
                                        k: Cont::CompoundRest {
                                            rest,
                                            chain,
                                            idx,
                                            node,
                                            k,
                                        },
                                    });
                                }
                                SimpleCheck::Has { lists, rest } => {
                                    try_has_alternative(
                                        &mut stack,
                                        lists,
                                        node,
                                        Cont::CompoundRest {
                                            rest,
                                            chain,
                                            idx,
                                            node,
                                            k,
                                        },
                                    )?;
                                }
                            },
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
                        stack.push(Frame::Deliver(true, *k));
                    } else {
                        match parent_element(from) {
                            Some(p) => stack.push(Frame::EvalCompound {
                                chain: chain.clone(),
                                idx,
                                node: p,
                                k: Cont::AncestorRetry {
                                    chain,
                                    idx,
                                    from: p,
                                    k,
                                },
                            }),
                            None => stack.push(Frame::Deliver(false, *k)),
                        }
                    }
                }
                Cont::SiblingRetry {
                    chain,
                    idx,
                    from,
                    k,
                } => {
                    if result {
                        stack.push(Frame::Deliver(true, *k));
                    } else {
                        match prev_sibling_element(from) {
                            Some(s) => stack.push(Frame::EvalCompound {
                                chain: chain.clone(),
                                idx,
                                node: s,
                                k: Cont::SiblingRetry {
                                    chain,
                                    idx,
                                    from: s,
                                    k,
                                },
                            }),
                            None => stack.push(Frame::Deliver(false, *k)),
                        }
                    }
                }
                Cont::AlternativeRetry {
                    lists,
                    node,
                    negate,
                    k,
                } => {
                    if result {
                        // This alternative matched: :is/:where succeeds
                        // (deliver `true`); :not is disproved by any match
                        // (deliver `false`). Either way, short-circuit -
                        // remaining alternatives don't need trying.
                        stack.push(Frame::Deliver(!negate, *k));
                    } else {
                        try_alternative(&mut stack, lists, node, negate, *k);
                    }
                }
                Cont::HasCandidateChecked {
                    chain,
                    idx,
                    cursor,
                    candidate,
                    k,
                } => {
                    if result {
                        if idx + 1 == chain.len() {
                            // This candidate satisfied the WHOLE :has()
                            // chain - the alternative succeeds.
                            stack.push(Frame::Deliver(true, *k));
                        } else {
                            // Descend to chain[idx + 1] FROM this candidate;
                            // a failure down there resumes `cursor` here
                            // (HasBacktrack), never a native call.
                            let next_cursor = HasCursor::start(chain[idx + 1].comb, candidate)?;
                            stack.push(Frame::HasStep {
                                chain: Rc::clone(&chain),
                                idx: idx + 1,
                                cursor: next_cursor,
                                k: Cont::HasBacktrack {
                                    chain,
                                    idx,
                                    cursor,
                                    k,
                                },
                            });
                        }
                    } else {
                        // This candidate's own compound didn't match; try
                        // the next one at the SAME level.
                        stack.push(Frame::HasStep {
                            chain,
                            idx,
                            cursor,
                            k: *k,
                        });
                    }
                }
                Cont::HasBacktrack {
                    chain,
                    idx,
                    cursor,
                    k,
                } => {
                    if result {
                        stack.push(Frame::Deliver(true, *k));
                    } else {
                        // The deeper search found nothing from THIS
                        // candidate; try the next one at THIS level.
                        stack.push(Frame::HasStep {
                            chain,
                            idx,
                            cursor,
                            k: *k,
                        });
                    }
                }
                Cont::HasAlternativeRetry { lists, node, k } => {
                    if result {
                        stack.push(Frame::Deliver(true, *k));
                    } else {
                        try_has_alternative(&mut stack, lists, node, *k)?;
                    }
                }
            },
        }
    }
    Ok(answer)
}

/// Does `element` match the selector chain starting at `first` (as
/// `css_parser` links it, left to right - i.e. `first` is the LEFTMOST
/// compound as written)? A fresh [`Budget`] each call - see [`MatchFailure`]'s
/// doc for why a failure is raised, not answered as a plain `false`.
pub fn matches(
    first: Option<Selector<'_>>,
    element: HtmlElement<'_>,
) -> Result<bool, MatchFailure> {
    let Some(compounds) = collect_compounds(first) else {
        return Ok(false);
    };
    let idx = compounds.len() - 1;
    let node = element.node();
    run(
        vec![Frame::EvalCompound {
            chain: compounds,
            idx,
            node,
            k: Cont::Root,
        }],
        &Budget::new(),
    )
}

/* ------------------------------------------------------------------ *
 * whole-query entry points: matches_any / select_all / select_first,  *
 * over a full (possibly comma-separated) `Lists` rather than one      *
 * chain - what `Node#{matches?,css,at_css}` each want                 *
 * ------------------------------------------------------------------ */

/// Does `element` match any comma-separated alternative of `groups`? The
/// entry point for `Node#matches?`: no traversal, just [`list_matches`]
/// (already the machinery `:is`/`:where` use internally) applied to the
/// query's own top-level groups, under a fresh [`Budget`].
pub fn matches_any(groups: Lists<'_>, element: HtmlElement<'_>) -> Result<bool, MatchFailure> {
    list_matches(groups, element.node(), &Budget::new())
}

/// Why [`select_all`]/[`select_first`]/[`matches_any`]/[`matches`] stopped
/// before answering the whole query. [`select_all`]'s own [`Overflow`] plus
/// whatever [`MatchFailure`] carries - see its doc for the other two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFailure {
    /// More descendants matched than a Makiri result set is allowed to hold.
    Overflow,
    /// The per-query work budget ran out - see [`Budget`]'s doc (mainly a
    /// `:has()` search costing more than the input's own size bounds).
    WorkExceeded,
    /// See [`MatchFailure::Unsupported`].
    Unsupported,
}

impl From<MatchFailure> for QueryFailure {
    #[inline]
    fn from(e: MatchFailure) -> Self {
        match e {
            MatchFailure::WorkExceeded => QueryFailure::WorkExceeded,
            MatchFailure::Unsupported => QueryFailure::Unsupported,
        }
    }
}

/// Every ELEMENT in `root`'s subtree - descendants only, `root` itself
/// excluded, exactly as the Lexbor-backed engine's `find` does (see
/// `lexbor::selectors::find_cb`) - that matches any alternative of `groups`,
/// in document order. A node matching more than one comma alternative is
/// reported once: `list_matches` is a plain OR over the alternatives for
/// ONE node, so no `MATCH_FIRST`-style dedup flag is needed the way
/// Lexbor's C API wants one.
///
/// `<template>` contents are not entered (`preorder_next`, not
/// `preorder_next_with_contents`) - the DOM's own rule for a descendant
/// walk, which `children`/`content=` already follow.
///
/// Capped at [`NODE_SET_MAX`], matching every other Makiri result set, and
/// at one shared [`Budget`] for the whole call (its doc). `falloc` is not
/// yet used for the result vector (see the module doc): the same tracked
/// gap as everywhere else in this file, not a new one.
pub fn select_all<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    select_all_with_budget(root, groups, &Budget::new())
}

/// [`select_all`]'s body, taking its [`Budget`] rather than making one - so
/// `#[cfg(test)]`'s [`select_all_with_work_limit`] can hand it a small one to
/// exercise the "the budget actually stops something" path without an
/// enormous fixture (10 million steps is right to ship, wrong to build a
/// document around in a test).
fn select_all_with_budget<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
    budget: &Budget,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    // Hoisted once for the whole tree walk below - see `CompiledList`'s doc.
    let compiled = compile_list(groups);
    let mut out = Vec::new();
    let mut n = root;
    while let Some(next) = n.preorder_next(root) {
        n = next;
        if n.element().is_some() && list_matches_compiled(&compiled, n, budget)? {
            if out.len() >= NODE_SET_MAX {
                return Err(QueryFailure::Overflow);
            }
            out.push(n);
        }
    }
    Ok(out)
}

/// Test-only: [`select_all`] with a caller-chosen work-budget limit instead
/// of [`DEFAULT_WORK_BUDGET`], so a test can prove the budget actually stops
/// an expensive `:has()` search without needing a document big enough to
/// exhaust the real, shipped limit.
#[cfg(test)]
pub(crate) fn select_all_with_work_limit<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
    limit: u64,
) -> Result<Vec<HtmlNode<'doc>>, QueryFailure> {
    select_all_with_budget(
        root,
        groups,
        &Budget {
            spent: std::cell::Cell::new(0),
            limit,
        },
    )
}

/// The first descendant of `root`, in document order, that matches any
/// alternative of `groups` - `root` itself excluded. Stops at the first hit
/// instead of building the whole set, as `Node#at_css` wants (see
/// `lexbor::selectors::first_cb`). One [`Budget`] for the whole search.
pub fn select_first<'doc>(
    root: HtmlNode<'doc>,
    groups: Lists<'_>,
) -> Result<Option<HtmlNode<'doc>>, MatchFailure> {
    let budget = Budget::new();
    // Hoisted once for the whole search below - see `CompiledList`'s doc.
    let compiled = compile_list(groups);
    let mut n = root;
    while let Some(next) = n.preorder_next(root) {
        n = next;
        if n.element().is_some() && list_matches_compiled(&compiled, n, &budget)? {
            return Ok(Some(n));
        }
    }
    Ok(None)
}
