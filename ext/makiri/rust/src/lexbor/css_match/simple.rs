//! One simple selector's verdict at one node ([`check_simple`]): type, id,
//! class, attribute and the plain pseudo-classes answered here, the ones
//! that nest a selector list deferred to a task (`query`). Names are
//! resolved once per query ([`Name`]).

use crate::lexbor::adapter::html::{
    has_ascii_uppercase, AttrName, HtmlAttr, HtmlDoc, HtmlElement, HtmlNode, NsId, TagId,
};
use crate::lexbor::css_parser::{AttrMatch, FunctionArg, PseudoClass, Simple};
use core::ffi::c_long;

use super::compile::{Nest, Step};
use super::positions::{sibling_position, Positions};
use super::scratch::Table;
use super::state::{
    has_state_attr, is_any_link, is_blank, is_checked, is_disabled, is_empty, is_enabled,
    is_html_namespace, is_lexbor_whitespace, is_placeholder_shown, is_read_write, is_required,
    is_root,
};
use super::tree::{next_position_sibling, prev_position_sibling};
use super::{Budget, MatchFailure};

/// Lexbor's `compat_mode` values (`adapter::html::HtmlDoc::compat_mode`'s own
/// doc: "0 no-quirks, 1 quirks, 2 limited-quirks").
fn document_is_quirks(node: HtmlNode<'_>) -> bool {
    node.owner_document().compat_mode() == 1
}

fn eq_bytes(a: &[u8], b: &[u8], case_insensitive: bool) -> bool {
    if case_insensitive {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Lexbor's `lxb_selectors_match_class` - whitespace-tokenize `target` and look
/// for a token equal to `want`. Also the engine behind `~=`
/// (`lxb_selectors_match_attribute`'s `INCLUDE`), which is why this takes a
/// `case_insensitive` flag rather than baking in quirks mode itself - the
/// caller decides which rule supplies it (document quirks mode for a bare class
/// selector, `i`/`s`/the HTML case-insensitive attribute table for `~=`).
fn has_whitespace_token(target: &[u8], want: &[u8], case_insensitive: bool) -> bool {
    if want.is_empty() {
        return false; // an empty class name/token never matches, as in Lexbor
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

/// A type selector `want`, compared as bytes (`matches?`, which resolves no
/// ids): the case-folded comparison Lexbor makes, then [`written_case_holds`].
fn name_eq(node: HtmlNode<'_>, want: &[u8]) -> bool {
    node.element().is_some_and(|el| {
        el.local_name().eq_ignore_ascii_case(want) && written_case_holds(node, el, want)
    })
}

/// Given that `el`'s name equals the type selector `want` with ASCII case
/// folded - which is all a Lexbor tag id says - whether it equals it as
/// Selectors 4 and the HTML Standard ("case-sensitivity of selectors") compare:
/// for an HTML element, `want` lower-cased against its localName; for any
/// other, `want` as written against its localName.
///
/// `lxb_selectors_match_element` folds case on every element, so
/// `fegaussianblur` matched SVG `feGaussianBlur`, and `div` an element made
/// `DIV` by createElementNS - neither of which a browser matches. The fold
/// stays as the first test, since it is the tag-id compare a walking query
/// makes; this only confirms it. An element with no written name - nearly
/// every one the parser makes - has its lower-cased stored name as its
/// localName, so no name is read: the fold is the answer on an HTML element,
/// and on another it holds exactly when `want` has no upper case.
///
/// Only that first case is inline. With the whole test inline, every plain
/// type scan (`li`, `ul li a`) measured 5-10% slower, on an HTML element that
/// never gets past the first branch.
#[inline]
fn written_case_holds(node: HtmlNode<'_>, el: HtmlElement<'_>, want: &[u8]) -> bool {
    (!el.has_written_name() && node.ns_id() == Some(NsId::HTML))
        || written_case_holds_slow(node, el, want)
}

#[inline(never)]
fn written_case_holds_slow(node: HtmlNode<'_>, el: HtmlElement<'_>, want: &[u8]) -> bool {
    if !el.has_written_name() {
        /* A foreign element (an HTML one answered inline), whose localName is
         * its stored, lower-cased name: equal to `want` as written, given the
         * fold, exactly when `want` has no upper case. Reading the name to
         * compare (`el.local_name() == want`) says the same and measured 8%
         * slower on an SVG scan - a Lexbor call per candidate. */
        return !has_ascii_uppercase(want);
    }
    let local = el.dom_local_name();
    if node.ns_id() == Some(NsId::HTML) {
        /* `want` lower-cased against the localName. Given the fold, that
         * holds exactly when the localName has no upper case - and needs no
         * lower-cased copy of `want`. */
        !has_ascii_uppercase(local)
    } else {
        local == want
    }
}

/// `lxb_selectors_match_attribute`: `[name op value]` (or `[name]`, existence,
/// when `at.value` is `None`). `value_ci` is [`Step::value_ci`]: `None` - a
/// name in Lexbor's case-insensitive table with no `i`/`s` - compares
/// case-insensitively on an HTML-namespace element.
///
/// Lexbor also asks that the owner document be an HTML document, a raw
/// document-type read `css_match` (which forbids `unsafe`) cannot make; it
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
        return true; // `[name]`: existence only
    };
    let ci = value_ci.unwrap_or_else(|| is_html_namespace(node));
    match op {
        AttrMatch::Equal => eq_bytes(value, want, ci),
        // Lexbor's `~=` literally reuses the class-token matcher.
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
        PseudoClass::Disabled => is_disabled(node, budget)?,
        PseudoClass::Enabled => is_enabled(node, budget)?,
        PseudoClass::Optional => is_required(node, false),
        PseudoClass::Required => is_required(node, true),
        PseudoClass::ReadOnly => !is_read_write(node, budget)?,
        PseudoClass::ReadWrite => is_read_write(node, budget)?,
        PseudoClass::Active => has_state_attr(node, b"active"),
        PseudoClass::Focus => has_state_attr(node, b"focus"),
        PseudoClass::Hover => has_state_attr(node, b"hover"),
        PseudoClass::PlaceholderShown => is_placeholder_shown(node),
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

/// `lxb_selectors_anb_calc`: is `pos` = `a*n + b` for some `n >= 0`? Exact,
/// where Lexbor divides in `double` - past 2^53 every `double` is an integer,
/// so its divisibility test there always passes (module doc).
///
/// In `i128`: `a` and `b` reach `LONG_MAX` in magnitude (Lexbor clamps them
/// there) and `pos` is a `u64`, so `pos - b` overflows 64 bits - and a
/// release build checks overflow, so `:nth-child(n-9223372036854775807)`
/// panicked. Nothing here can overflow 128 bits: `|k| < 2^65`. Treating an
/// overflow as "no match" instead would be a wrong answer, not a safe one -
/// that selector (a = 1) matches every element.
pub(super) fn anb_matches(a: c_long, b: c_long, pos: u64) -> bool {
    let (a, b, pos) = (i128::from(a), i128::from(b), i128::from(pos));
    if a == 0 {
        return pos == b;
    }
    let k = pos - b;
    k % a == 0 && k / a >= 0
}

/// One simple selector's verdict at a node, or `Deferred`: it is a
/// `:is()`/`:where()`/`:not()`/`:has()`/`of S`, which a nested [`Task`](super::query::Task)
/// answers.
pub(super) enum SimpleCheck {
    Result(bool),
    Deferred,
}

#[inline]
pub(super) fn check_simple(
    sel: &Step<'_>,
    name: Name,
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
        // `lxb_selectors_match_id` / `_class` through the element's own `id` /
        // `class` shortcut, as Lexbor reads them (`HtmlElement::id_attr`) - no
        // attribute-list scan.
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
        // `lxb_selectors_match_attribute`: explicit `i` -> case-insensitive;
        // explicit `s` -> forced case-sensitive; no modifier -> the HTML table
        // decides. `compile` settled which in `Step::value_ci`.
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
        }) => match sel.nest {
            Nest::None => SimpleCheck::Result(nth_matches(
                node, from_end, of_type, anb, budget, positions,
            )?),
            _ => SimpleCheck::Deferred,
        },
        Simple::PseudoClassFunction(FunctionArg::Selectors { .. }) => SimpleCheck::Deferred,
        // `:lexbor-contains()`: Lexbor itself matches with it
        // (`lxb_selectors_pseudo_class_function`'s `LEXBOR_CONTAINS`) - this
        // port deliberately does not (`MatchFailure::Unsupported`'s doc) - so
        // answering `false` would be indistinguishable from a selector that
        // legitimately matches nothing. Raised instead.
        Simple::PseudoClassFunction(FunctionArg::Contains(_)) => {
            return Err(MatchFailure::Unsupported)
        }
        // Any OTHER functional pseudo-class (`:dir()`, `:lang()`, `:nth-col()`,
        // `:nth-last-col()`) is unimplemented in LEXBOR TOO
        // (`lxb_selectors_pseudo_class_function`'s `default:` case) - a real,
        // agreed "always false", not a gap this port introduces.
        Simple::PseudoClassFunction(FunctionArg::Other) => SimpleCheck::Result(false),
        Simple::PseudoElement | Simple::Other => SimpleCheck::Result(false),
    })
}

/// A simple selector's name resolved in the document a walking query walks,
/// the first time a candidate reaches it, and kept for the rest of the
/// query - Lexbor's own lazily set `entry->id` - for the kinds that look a
/// name up on every candidate. Lexbor keys element and attribute names by
/// their ASCII lower-cased form, so an id match is exactly the case-folded
/// comparison the byte path makes (a type selector), or a necessary
/// condition that the adapter then confirms (an attribute:
/// `attr_by_resolved_name`).
///
/// A resolved name - tag id or attribute id - is used WITHOUT asking each
/// node for its document, as Lexbor's `entry->id` is: every node a walking
/// query reaches - the walk itself, a combinator's climb, `:has()`'s search,
/// `of S`'s siblings - is in the walked tree, so of the walked document,
/// since Makiri never moves a node between documents (inserting one from
/// another document inserts a copy, `bridge::html::insert`). The attribute
/// side is `attr_by_resolved_name`'s precondition, asserted in debug builds.
/// The per-node check this replaced read `owner_document` on every
/// candidate, which cost `css("li")` ~10% on a document not in cache.
#[derive(Clone, Copy, Default)]
pub(super) enum Name {
    /// Not looked up yet.
    #[default]
    Unresolved,
    /// Nothing to look up: not a type or attribute selector.
    None,
    /// A type selector's tag id in the walked document; `None`: no element
    /// of the document has the name.
    Tag(Option<TagId>),
    /// An attribute selector's name.
    Attr(AttrName),
}

impl Name {
    pub(super) fn resolve(sel: &Step<'_>, doc: HtmlDoc<'_>) -> Name {
        match sel.simple {
            Simple::Type => Name::Tag(doc.tag_id(sel.name)),
            Simple::Attribute(_) => Name::Attr(doc.resolve_attr_name(sel.name)),
            _ => Name::None,
        }
    }
}

/// A query's [`Name`]s, by simple-selector index: one per simple
/// selector, each resolved when it is first reached.
pub(super) type Names = Table<Name>;

/// `lxb_selectors_match_element` through [`Name`]: one id comparison in a
/// walking query (the node is of the walked document - [`Name`]'s doc),
/// [`name_eq`] otherwise; either way confirmed by [`written_case_holds`].
#[inline]
fn type_matches(node: HtmlNode<'_>, want: &[u8], name: Name) -> bool {
    match name {
        Name::Tag(id) => {
            id.is_some()
                && node.tag_id() == id
                && node
                    .element()
                    .is_some_and(|el| written_case_holds(node, el, want))
        }
        _ => name_eq(node, want),
    }
}

/// The value of `node`'s attribute `qname` (DOM `getAttribute`), through
/// the resolved [`Name`] when there is one.
#[inline]
fn attr_value<'doc>(node: HtmlNode<'doc>, qname: &[u8], name: Name) -> Option<&'doc [u8]> {
    let el = node.element()?;
    match name {
        Name::Attr(resolved) => el
            .attr_by_resolved_name(qname, resolved)
            .map(HtmlAttr::value),
        _ => el.get_attribute(qname),
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
