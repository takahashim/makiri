//! CSS selector matcher over the typed HTML adapter, structured as an
//! explicit (heap) work stack rather than native recursion - the same
//! architectural property Lexbor's own `lxb_selectors_*` state machine has
//! (`notes/css_selectors_crate_migration_plan.ja.md` §1.1: `do { entry =
//! selectors->state(...) } while (entry != NULL);` never recurses on
//! selector nesting). Selector PARSING is not reimplemented: it goes through
//! the existing `lexbor::css_parser` typed view, the same one the XML
//! CSS->XPath lowering already uses.
//!
//! Not wired into `Node#css`/`#at_css`/`#matches?` yet - see
//! `lexbor::selector_element` (the rejected (A) exploration) for why (B) was
//! chosen instead.
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
//! `:has()`'s own compound-by-compound forward search (§A-4, `has_matches`)
//! DOES use ordinary Rust recursion, unlike everything else here - and that
//! is a deliberate, bounded exception, not an oversight: the recursion depth
//! there is bounded by `MAX_COMPOUNDS` (64, a fixed complexity cap already
//! enforced by `collect_compounds`), never by attacker-controlled selector
//! NESTING the way `:is()` is. **Measured, not just argued**: a `:has()`
//! argument built to force exactly 64 levels of `has_forward` (a distinct
//! class per level, so no early mismatch can cut it short) still answers
//! correctly on a thread given only 64 KiB of stack, and only overflows at
//! 32 KiB - a >=2x margin below Ruby's smallest documented `Fiber` machine
//! stack (`RUBY_FIBER_MACHINE_STACK_SIZE`, as small as 128 KiB - see
//! `crate::stack`'s module doc), and in an unoptimized DEBUG build, which
//! uses far more stack per frame than the release build this ships as
//! (`lexbor::tests::selector_port_spike::has_forward_at_max_compounds_fits_the_smallest_fiber_stack`).
//! That test cannot exercise being called from PARTWAY into a Fiber's own
//! stack (this port has no Ruby entry point yet to do that through) - the
//! number is a lower bound on the margin, not the in-Ruby one - but it
//! answers what Phase 1 could only argue: 64 native frames really is
//! negligible, not merely assumed to be.

#![forbid(unsafe_code)]

use crate::lexbor::adapter::html::{HtmlElement, HtmlNode, NsId};
use crate::lexbor::css_parser::{
    AttrMatch, Combinator, FunctionArg, ListPseudo, Lists, PseudoClass, Selector, Simple,
};
use crate::limits::NODE_SET_MAX;

/// A complexity bound on compounds per chain, mirroring `css::MAX_COMPOUNDS`.
///
/// Not itself a stack-safety mechanism (this design needs none for nesting) -
/// just a sanity cap so a single compound chain can't grow unboundedly, AND
/// (see the module doc) what makes `:has()`'s bounded recursion safe.
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
/// input document/selector's own size bounds already (concretely: each node
/// [`has_forward`]'s own search visits, and each frame [`run`]'s trampoline
/// pops) charges it once. Exceeding it is [`WorkExceeded`] - a hard stop
/// propagated all the way back to the caller, never a silent `false` for
/// just the one `:has()` that happened to hit it: a `:has()` inside a larger
/// compound answering `false` because ITS OWN search ran out of budget would
/// be a wrong verdict for that compound, not merely an incomplete one, and
/// CLAUDE.md's fail-closed rule is "raise instead" of that.
///
/// A plain `Cell`, not `xpath::limits::Budget`'s `Rc<RefCell<Error>>` sink:
/// this file has no Ruby-facing diagnostic error to build yet (not wired in -
/// module doc), and one `u64` counter local to a single call needs no shared
/// ownership.
struct Budget {
    spent: std::cell::Cell<u64>,
    limit: u64,
}

/// [`Budget`] ran out: some step - most likely `:has()`'s own search -
/// charged past its call's cap. Propagated as a hard failure, not folded into
/// a `bool`; see [`Budget`]'s doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkExceeded;

impl Budget {
    fn new() -> Self {
        Budget {
            spent: std::cell::Cell::new(0),
            limit: DEFAULT_WORK_BUDGET,
        }
    }

    /// Charge one step. `Err(WorkExceeded)` once the limit is reached - the
    /// caller must stop and propagate it, not answer as if this step had
    /// simply failed to match.
    #[inline]
    fn charge(&self) -> Result<(), WorkExceeded> {
        let n = self.spent.get() + 1;
        self.spent.set(n);
        if n > self.limit {
            return Err(WorkExceeded);
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

/// Split a chain into compounds, left to right (the order `css_parser` links
/// them in). `None` for an empty chain or one over `MAX_COMPOUNDS`.
fn collect_compounds(first: Option<Selector<'_>>) -> Option<Vec<Compound<'_>>> {
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
        Some(v)
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
) -> Result<u64, WorkExceeded> {
    let same_type = |n: HtmlNode<'_>| !of_type || name_matches_type(n, node);
    let mut pos: u64 = 1;
    let mut cur = if from_end {
        next_sibling_element(node)
    } else {
        prev_sibling_element(node)
    };
    while let Some(n) = cur {
        budget.charge()?;
        if same_type(n) {
            pos += 1;
        }
        cur = if from_end {
            next_sibling_element(n)
        } else {
            prev_sibling_element(n)
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
) -> Result<Option<u64>, WorkExceeded> {
    if !list_matches(list, node, budget)? {
        return Ok(None);
    }
    let mut pos: u64 = 1;
    let mut cur = if from_end {
        next_sibling_element(node)
    } else {
        prev_sibling_element(node)
    };
    while let Some(n) = cur {
        if list_matches(list, n, budget)? {
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
) -> Result<bool, WorkExceeded> {
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
/// `of S` and by `:has()`'s per-compound checks (`has_matches`) so both go
/// through the exact same `:is`/`:where`/`:not`-nesting-safe machinery
/// `matches` does, rather than a second, weaker implementation.
fn matches_one_compound_chain(
    first: Selector<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, WorkExceeded> {
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

fn is_empty(node: HtmlNode<'_>) -> bool {
    // §C-1 `:empty`: no element child, and no non-empty text/CDATA child
    // (comments don't count either way).
    !node
        .children()
        .any(|c| c.element().is_some() || c.char_data().is_some_and(|t| !t.is_empty()))
}

/// §C-1 `:blank`: as `:empty`, but whitespace-only text children don't
/// disqualify it (Lexbor's `lxb_dom_node_is_empty`) - more permissive than
/// `:empty`.
fn is_blank(node: HtmlNode<'_>) -> bool {
    !node.children().any(|c| {
        c.element().is_some()
            || c.char_data()
                .is_some_and(|t| t.iter().any(|&b| !is_lexbor_whitespace(b)))
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
) -> Result<bool, WorkExceeded> {
    Ok(match pc {
        PseudoClass::FirstChild => prev_sibling_element(node).is_none(),
        PseudoClass::LastChild => next_sibling_element(node).is_none(),
        PseudoClass::OnlyChild => {
            prev_sibling_element(node).is_none() && next_sibling_element(node).is_none()
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
) -> Result<bool, WorkExceeded> {
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
) -> Result<bool, WorkExceeded> {
    let Some(anb) = anb else {
        return Ok(false);
    };
    let Some(pos) = sibling_position_of(node, from_end, list, budget)? else {
        return Ok(false);
    };
    Ok(anb_matches(anb, pos as i64))
}

/* ------------------------------------------------------------------ *
 * :has() - §A-4: forward search, bounded recursion (module doc)      *
 * ------------------------------------------------------------------ */

/// §A-4: does ANY alternative of `lists` (`:has(a, b)` = OR) match, relative
/// to `node`?
fn has_matches(
    lists: Lists<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, WorkExceeded> {
    for list in lists {
        if let Some(first) = list.first() {
            if has_matches_one(first, node, budget)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// One `:has()` alternative: `first` is the chain's ANCHOR (leftmost, as
/// written), whose OWN combinator connects it to `node` (Lexbor sets this to
/// Descendant by default when no leading combinator was written -
/// `notes/lexbor_selectors_c_semantics.ja.md` §A-4). Bounded recursion over
/// `MAX_COMPOUNDS` compounds - see the module doc for why that's safe here.
fn has_matches_one(
    first: Selector<'_>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, WorkExceeded> {
    let Some(compounds) = collect_compounds(Some(first)) else {
        return Ok(false);
    };
    has_forward(&compounds, 0, node, budget)
}

/// `:has()`'s own search, over EVERY candidate `comb` reaches from `node` -
/// the one place a `:has()` argument's cost can multiply per candidate
/// element of a broader query, rather than being bounded by the input's own
/// size the way everything else here is (module doc, `Budget`'s doc). Every
/// candidate visited - matched or not - charges `budget` once, and a charge
/// failure aborts the search immediately rather than answering as if no
/// candidate had matched.
fn has_forward(
    chain: &[Compound<'_>],
    idx: usize,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, WorkExceeded> {
    let compound = chain[idx];
    let is_last = idx + 1 == chain.len();
    let candidate_ok = |c: HtmlNode<'_>| -> Result<bool, WorkExceeded> {
        budget.charge()?;
        Ok(matches_compound_here(compound, c, budget)?
            && (is_last || has_forward(chain, idx + 1, c, budget)?))
    };
    match compound.comb {
        // The whole subtree - `HtmlNode::subtree`'s own doc: "climbs by
        // parent links rather than recursing", so an adversarially deep OR
        // wide document cannot exhaust the stack here either (`budget`
        // bounds its cost instead).
        Combinator::Descendant => {
            for c in node.subtree().skip(1) {
                if candidate_ok(c)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Combinator::Child => {
            let mut cur = node.first_child();
            while let Some(c) = cur {
                if c.element().is_some() && candidate_ok(c)? {
                    return Ok(true);
                }
                cur = c.next();
            }
            Ok(false)
        }
        Combinator::NextSibling => match next_sibling_element(node) {
            Some(c) => candidate_ok(c),
            None => Ok(false),
        },
        Combinator::SubsequentSibling => {
            let mut cur = next_sibling_element(node);
            while let Some(s) = cur {
                if candidate_ok(s)? {
                    return Ok(true);
                }
                cur = next_sibling_element(s);
            }
            Ok(false)
        }
        Combinator::Close | Combinator::Other => Ok(false),
    }
}

/// A single compound (no further leftward chain) against `c`, including any
/// `:is`/`:where`/`:not` it carries - via [`matches_one_compound_chain`], so
/// `:has(:is(a))`-style nesting inside a `:has()` argument's compound is
/// resolved through the same nesting-safe machine as everywhere else.
fn matches_compound_here(
    compound: Compound<'_>,
    c: HtmlNode<'_>,
    budget: &Budget,
) -> Result<bool, WorkExceeded> {
    // Deliberately NOT `matches_one_compound_chain(compound.first, c)`: that
    // re-derives the chain via `collect_compounds`, which walks PAST this
    // one compound to whatever the ORIGINAL selector chained after it (e.g.
    // for `:has(section > p.x)`, `compound` is "section" alone, but
    // `compound.first.next()` is "p.x" via a `Child` combinator - re-parsing
    // from there would ask "is `c` a `p.x` whose parent is `section`",
    // exactly the wrong question `has_forward` already answers one level up).
    // Wrapping `compound` in a length-1 chain checks ONLY its own simple
    // selectors (including any `:is`/`:where`/`:not` it carries, still fully
    // nesting-safe via `run`), never following `.first.next()`.
    run(
        vec![Frame::EvalCompound {
            chain: vec![Compound {
                first: compound.first,
                comb: Combinator::Close,
            }],
            idx: 0,
            node: c,
            k: Cont::Root,
        }],
        budget,
    )
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
}

fn check_simple<'p>(
    sel: Selector<'p>,
    node: HtmlNode<'_>,
    budget: &Budget,
) -> Result<SimpleCheck<'p>, WorkExceeded> {
    Ok(match sel.simple() {
        Simple::Universal => SimpleCheck::Result(true),
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
        }) => SimpleCheck::Result(has_matches(lists, node, budget)?),
        Simple::PseudoClassFunction(FunctionArg::Selectors { pseudo, lists }) => {
            SimpleCheck::Defer {
                negate: pseudo == ListPseudo::Not,
                lists,
                // `check_from` (not this function) knows the compound's
                // chain and fills the real value in.
                rest: None,
            }
        }
        // `:lexbor-contains()` and any other functional pseudo-class are
        // simply unsupported here.
        Simple::PseudoClassFunction(FunctionArg::Contains(_) | FunctionArg::Other) => {
            SimpleCheck::Result(false)
        }
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
) -> Result<SimpleCheck<'p>, WorkExceeded> {
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
) -> Result<SimpleCheck<'p>, WorkExceeded> {
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
        chain: Vec<Compound<'p>>,
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
        chain: Vec<Compound<'p>>,
        idx: usize,
        node: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// Retry `chain[idx]` at the next ancestor of `from` on failure; forward
    /// success as-is.
    AncestorRetry {
        chain: Vec<Compound<'p>>,
        idx: usize,
        from: HtmlNode<'doc>,
        k: Box<Cont<'p, 'doc>>,
    },
    /// As `AncestorRetry`, over preceding sibling elements (`~`).
    SiblingRetry {
        chain: Vec<Compound<'p>>,
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
}

/// After `chain[idx]` is known to match (or not) at `node`, do what an
/// ordinary (non-deferred) hit would: on failure, fail the whole chain; on
/// success, continue left (or report success, at `idx == 0`).
fn advance<'p, 'doc>(
    stack: &mut Vec<Frame<'p, 'doc>>,
    chain: Vec<Compound<'p>>,
    idx: usize,
    node: HtmlNode<'doc>,
    k: Cont<'p, 'doc>,
    matched: bool,
) {
    if !matched {
        stack.push(Frame::Deliver(false, k));
        return;
    }
    if idx == 0 {
        stack.push(Frame::Deliver(true, k));
        return;
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
        Combinator::Other => stack.push(Frame::Deliver(false, k)), // column combinator etc.: fail closed
    }
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

/// Run the machine to completion. `stack` starts with exactly one frame; the
/// loop is the WHOLE control flow - no Rust-level recursion anywhere here,
/// regardless of how deeply the selector nests. Charges `budget` once per
/// frame popped - an ancestor/sibling retry that failed and is trying the
/// next candidate re-enters this loop with a fresh frame, so a `:is()`
/// alternative or a long ancestor climb is bounded by the same budget
/// `:has()`'s own search is, not just by depth counts.
fn run(mut stack: Vec<Frame<'_, '_>>, budget: &Budget) -> Result<bool, WorkExceeded> {
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
                SimpleCheck::Result(m) => advance(&mut stack, chain, idx, node, k, m),
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
            },
            Frame::TryAlternatives {
                lists,
                node,
                negate,
                k,
            } => try_alternative(&mut stack, lists, node, negate, k),
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
                        advance(&mut stack, chain, idx, node, *k, false);
                    } else {
                        match rest {
                            None => advance(&mut stack, chain, idx, node, *k, true),
                            Some(next) => match check_from(next, node, budget)? {
                                SimpleCheck::Result(m) => {
                                    advance(&mut stack, chain, idx, node, *k, m)
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
            },
        }
    }
    Ok(answer)
}

/// Does `element` match the selector chain starting at `first` (as
/// `css_parser` links it, left to right - i.e. `first` is the LEFTMOST
/// compound as written)? A fresh [`Budget`] each call - see its doc for why
/// exceeding it is [`WorkExceeded`], not a plain `false`.
pub fn matches(
    first: Option<Selector<'_>>,
    element: HtmlElement<'_>,
) -> Result<bool, WorkExceeded> {
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
pub fn matches_any(groups: Lists<'_>, element: HtmlElement<'_>) -> Result<bool, WorkExceeded> {
    list_matches(groups, element.node(), &Budget::new())
}

/// Why [`select_all`]/[`select_first`]/[`matches_any`]/[`matches`] stopped
/// before answering the whole query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFailure {
    /// More descendants matched than a Makiri result set is allowed to hold.
    Overflow,
    /// The per-query work budget ran out - see [`Budget`]'s doc (mainly a
    /// `:has()` search costing more than the input's own size bounds).
    WorkExceeded,
}

impl From<WorkExceeded> for QueryFailure {
    #[inline]
    fn from(_: WorkExceeded) -> Self {
        QueryFailure::WorkExceeded
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
    let mut out = Vec::new();
    let mut n = root;
    while let Some(next) = n.preorder_next(root) {
        n = next;
        if n.element().is_some() && list_matches(groups, n, budget)? {
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
) -> Result<Option<HtmlNode<'doc>>, WorkExceeded> {
    let budget = Budget::new();
    let mut n = root;
    while let Some(next) = n.preorder_next(root) {
        n = next;
        if n.element().is_some() && list_matches(groups, n, &budget)? {
            return Ok(Some(n));
        }
    }
    Ok(None)
}
