//! The lowering itself: a Lexbor selector chain becomes XPath steps and
//! predicates.
//!
//! - a COMPOUND (a run of simple selectors on one element) becomes ONE step: the
//!   type selector sets the node test, everything else appends a predicate;
//! - a COMBINATOR becomes the axis connecting one step to the next, except `+`,
//!   which needs two steps (`following-sibling::*[1]/self::b`);
//! - a functional pseudo-class that takes a selector (`:is`, `:not`, `:has`)
//!   lowers its argument back through here, which is why the two entry points
//!   below are mutually recursive with the pseudo lowering.
//!
//! `:is`/`:where`/`:not` need a BOOLEAN self-test rather than a path, so they go
//! through [`complex_selftest`], which walks the chain right to left using the
//! reverse axes. The path it builds is non-empty - hence truthy - exactly when
//! self matches the selector.
//!
//! The parsed selectors are read only through `css_parser`'s typed views, so
//! this module holds no pointer into Lexbor's arena, sees none of its enum
//! values, and forbids unsafe.

#![forbid(unsafe_code)]

use super::build::{self, Built};
use super::{Build, MAX_COMPOUNDS};
use crate::engine_error::{ErrorKind, Reported};
use crate::lexbor::css_parser::{
    AttrMatch, Attribute, Combinator, FunctionArg, ListPseudo, Lists, Nth, PseudoClass, Selector,
    Simple,
};
use crate::xpath::ast::{Axis, Expr, NodeTest, Op, Step};

/// The internal of-type position functions, whose names carry a leading \x01 so
/// no user expression can name them.
use crate::xpath::funcs::{
    FN_CHILD_POS, FN_CHILD_POS_LAST, FN_IS_TEXT, FN_OF_TYPE_POS, FN_OF_TYPE_POS_LAST,
};

/* ------------------------------------------------------------------ *
 * simple selectors                                                   *
 * ------------------------------------------------------------------ */

/// Set the step's name test from a type selector, honouring the CSS namespace
/// rules and the Nokogiri default-namespace binding.
fn lower_type(
    b: &Build,
    s: Selector<'_>,
    step: &mut Step,
    preds: &mut Vec<Expr>,
) -> Result<(), Reported> {
    let name = s.name();
    let ns = s.ns();

    if ns == Some(b"*") {
        /* `*|el`: any namespace with a specific local name. XPath has no such
         * test, so it becomes a wildcard plus a local-name() predicate. */
        step.test = NodeTest::ANY;
        let ln = build::call0(b, b"local-name");
        let lit = build::literal(b, name);
        return push_pred(b, preds, build::binop(b, Op::Eq, || ln, || lit));
    }

    let local = build::copy_text(b, name)?;

    let prefix = match ns {
        /* `p|el` */
        Some(p) if !p.is_empty() => Some(build::copy_text(b, p)?),
        /* `|el`: an explicit no-namespace, so leave the prefix unset. */
        Some(_) => None,
        /* A bare `el`. With a document default namespace in scope it binds to
         * the synthetic prefix, which is Nokogiri's behaviour. */
        None => match b.default_prefix() {
            Some(dp) => Some(build::copy_text(b, dp)?),
            None => None,
        },
    };
    step.test = NodeTest::Name { prefix, local };
    Ok(())
}

/// Set the step's test from a universal selector, honouring its namespace.
///
/// A bare `*` and `*|*` match any element - a bare `*` is not bound to the
/// default namespace, which keeps Nokogiri's reading. `p|*` is XPath's `p:*`,
/// and `|*` - no namespace, which XPath 1.0 has no test for - a wildcard plus a
/// `namespace-uri() = ''` predicate.
fn lower_universal(
    b: &Build,
    s: Selector<'_>,
    step: &mut Step,
    preds: &mut Vec<Expr>,
) -> Result<(), Reported> {
    step.test = NodeTest::ANY;
    match s.ns() {
        None | Some(b"*") => Ok(()),
        Some(b"") => {
            let uri = build::call0(b, b"namespace-uri");
            let lit = build::literal(b, b"");
            push_pred(b, preds, build::binop(b, Op::Eq, || uri, || lit))
        }
        Some(p) => {
            step.test = NodeTest::Wildcard {
                prefix: Some(build::copy_text(b, p)?),
            };
            Ok(())
        }
    }
}

/// `[name op value]` as an expression.
fn lower_attribute(b: &Build, s: Selector<'_>, at: Attribute<'_>) -> Built {
    let name = s.name();

    if at.case_insensitive {
        return Err(b.fail(
            ErrorKind::Syntax,
            "CSS attribute case modifier i ([a=v i]) is not supported for XML",
        ));
    }

    /* The attribute namespace: NULL is a bare name, anything else a prefix. An
     * unprefixed CSS attribute selector matches the no-namespace attribute, per
     * CSS and XPath alike. Lexbor stores `[|a]` - explicitly no namespace -
     * with the namespace `*`, and refuses `[*|a]` (any namespace) before it gets
     * here, so a `*` is `[|a]` and reads as no prefix. It was refused as
     * `[*|a]`. */
    let prefix = match s.ns() {
        Some(p) if p == b"*" => None,
        other => other,
    };

    let Some(value) = at.value else {
        /* `[name]` - existence */
        return build::attr_ns(b, prefix, name);
    };

    /* An empty value for ^= $= *= ~=, or a value holding whitespace for ~=,
     * "represents nothing" (Selectors 4 §6.2-6.3), as the HTML matcher has it.
     * Lowered as written they matched everything - starts-with(@a, '') is true
     * with no @a at all, so [z^=""] found every element. */
    let never = match at.op {
        AttrMatch::Prefix | AttrMatch::Suffix | AttrMatch::Substring => value.is_empty(),
        AttrMatch::Include => {
            value.is_empty()
                || value
                    .iter()
                    .any(|&c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
        }
        _ => false,
    };
    if never {
        return build::call0(b, b"false");
    }

    match at.op {
        /* [a=v] -> @a = 'v' */
        AttrMatch::Equal => build::binop(
            b,
            Op::Eq,
            || build::attr_ns(b, prefix, name),
            || build::literal(b, value),
        ),
        /* [a~=v] -> whitespace-separated token match */
        AttrMatch::Include => build::token_match(b, prefix, name, value),
        /* [a^=v] -> starts-with(@a, 'v') */
        AttrMatch::Prefix => build::call2(
            b,
            b"starts-with",
            || build::attr_ns(b, prefix, name),
            || build::literal(b, value),
        ),
        /* [a*=v] -> contains(@a, 'v') */
        AttrMatch::Substring => build::call2(
            b,
            b"contains",
            || build::attr_ns(b, prefix, name),
            || build::literal(b, value),
        ),
        /* [a$=v] -> substring(@a, string-length(@a) - len + 1) = 'v', where len
         * counts CHARACTERS: string-length and substring do, so a byte count
         * missed any non-ASCII suffix ([d$="é"]). */
        AttrMatch::Suffix => {
            let slen = build::call1(b, b"string-length", || build::attr_ns(b, prefix, name));
            let chars = crate::xpath::funcs::count_chars(value);
            let start = build::binop(
                b,
                Op::Add,
                || build::binop(b, Op::Sub, || slen, || build::num(b, chars as f64)),
                || build::num(b, 1.0),
            );
            let sub = build::call2(
                b,
                b"substring",
                || build::attr_ns(b, prefix, name),
                || start,
            );
            build::binop(b, Op::Eq, || sub, || build::literal(b, value))
        }
        /* [a|=v] -> @a = 'v' or starts-with(@a, 'v-') */
        AttrMatch::Dash => {
            let eq = build::binop(
                b,
                Op::Eq,
                || build::attr_ns(b, prefix, name),
                || build::literal(b, value),
            );
            let Some(mut dashed) = crate::falloc::try_vec_with_capacity::<u8>(value.len() + 1)
            else {
                return Err(b.oom());
            };
            dashed.extend_from_slice(value);
            dashed.push(b'-');
            let pre = build::call2(
                b,
                b"starts-with",
                || build::attr_ns(b, prefix, name),
                || build::literal(b, &dashed),
            );
            build::binop(b, Op::Or, || eq, || pre)
        }
        AttrMatch::Other => Err(b.fail(ErrorKind::Syntax, "unsupported CSS attribute operator")),
    }
}

/// `not(axis::nt)` - "nothing on that axis".
fn not_axis(b: &Build, axis: Axis, nt: NodeTest) -> Built {
    build::call1(b, b"not", || build::step_path(b, axis, nt))
}

/// `:root` - the document element.
///
/// Lexbor's matcher answers `:root` with
/// `lxb_dom_document_root(owner_document) == node`, so a detached element (no
/// parent at all) and a fragment's top element do NOT match. `not(parent::*)`
/// alone did match them: it takes "no ELEMENT parent" for "the root". Require a
/// parent NODE that is not an element - the document element's parent is the
/// document - so an orphan, which has no parent node, is excluded.
fn root_test(b: &Build) -> Built {
    let mut step = Step::new(Axis::Parent, NodeTest::Node);
    let parent_is_element = build::step_path(b, Axis::SelfAxis, NodeTest::ANY);
    build::push(
        b,
        &mut step.predicates,
        build::call1(b, b"not", || parent_is_element)?,
    )?;
    build::single_step_path(b, step)
}

/// The siblings a structural pseudo-class counts among.
///
/// Each is answered by an internal function reading a per-parent memo of
/// positions (`xpath::funcs::SiblingPositions`), not by counting a sibling
/// axis per candidate: `count(preceding-sibling::*)` per candidate is n^2 over
/// a flat list, and a 5,000-entry sitemap took 5-14 s for `:first-of-type` and
/// `:nth-child(2n)`.
#[derive(Clone, Copy)]
enum Siblings {
    /// Every element sibling: the `-child` family.
    All,
    /// Siblings with the element's own expanded name: the `-of-type` family.
    /// A typed compound (`a:nth-of-type`) is the same set - the candidate
    /// passed the type test, which names one expanded name (`*|a` is lowered
    /// to a wildcard plus a local-name predicate, not a name test), so its own
    /// name is the test's.
    SameType,
}

impl Siblings {
    /// The internal position function counting along `axis`: from the start
    /// when it looks back, from the end otherwise.
    fn position_fn(self, axis: Axis) -> &'static [u8] {
        let from_start = axis == Axis::PrecedingSibling;
        match (self, from_start) {
            (Siblings::All, true) => FN_CHILD_POS,
            (Siblings::All, false) => FN_CHILD_POS_LAST,
            (Siblings::SameType, true) => FN_OF_TYPE_POS,
            (Siblings::SameType, false) => FN_OF_TYPE_POS_LAST,
        }
    }
}

/// The 1-based position among `set`, counted along `axis`.
fn position(b: &Build, axis: Axis, set: Siblings) -> Built {
    build::call0(b, set.position_fn(axis))
}

/// "No sibling of `set` along `axis`" - first (looking back) or last (looking
/// forward) among them.
fn none_along(b: &Build, axis: Axis, set: Siblings) -> Built {
    build::binop(b, Op::Eq, || position(b, axis, set), || build::num(b, 1.0))
}

/// Both first and last among `set`: the `only-` family.
fn only(b: &Build, set: Siblings) -> Built {
    build::binop(
        b,
        Op::And,
        || none_along(b, Axis::PrecedingSibling, set),
        || none_along(b, Axis::FollowingSibling, set),
    )
}

/// The `:nth-*(an+b)` match condition over the position among `set` along
/// `axis`.
fn nth(b: &Build, axis: Axis, set: Siblings, anb: Nth<'_>) -> Built {
    /* `c_long` from Lexbor's `lxb_css_syntax_anb_t` - 64-bit on LP64, 32-bit on
     * LLP64 - so the `as f64` below is a real conversion on either. */
    let (a, bb) = (anb.a as f64, anb.b as f64);
    if anb.a == 0 {
        /* position = b */
        return build::binop(b, Op::Eq, || position(b, axis, set), || build::num(b, bb));
    }
    /* (pos - b) mod a == 0, plus "the index is not negative". The index is
     * (pos - b) / a, and a's sign is known HERE, at build time, so the sign
     * test is `pos >= b` for a > 0 and `pos <= b` for a < 0 - no division runs
     * per candidate. (For an exact multiple the quotient's sign is (pos - b)'s
     * times a's, so the two agree.) */
    let modz = build::binop(
        b,
        Op::Eq,
        || {
            build::binop(
                b,
                Op::Mod,
                || build::binop(b, Op::Sub, || position(b, axis, set), || build::num(b, bb)),
                || build::num(b, a),
            )
        },
        || build::num(b, 0.0),
    );
    let a_positive = anb.a > 0;
    let sign_ok = build::binop(
        b,
        if a_positive { Op::Ge } else { Op::Le },
        || position(b, axis, set),
        || build::num(b, bb),
    );
    build::binop(b, Op::And, || modz, || sign_ok)
}

/// The non-functional structural pseudo-classes.
fn lower_pseudo_simple(b: &Build, pc: PseudoClass) -> Built {
    let of_type = Siblings::SameType;
    match pc {
        PseudoClass::FirstChild => none_along(b, Axis::PrecedingSibling, Siblings::All),
        PseudoClass::LastChild => none_along(b, Axis::FollowingSibling, Siblings::All),
        PseudoClass::OnlyChild => only(b, Siblings::All),
        PseudoClass::FirstOfType => none_along(b, Axis::PrecedingSibling, of_type),
        PseudoClass::LastOfType => none_along(b, Axis::FollowingSibling, of_type),
        PseudoClass::OnlyOfType => only(b, of_type),
        /* No child but comments, as the HTML matcher (Lexbor) has it: an
         * element, text or processing instruction makes it non-empty. `not(node())`
         * counted a comment too, so <e><!--c--></e> was empty only in HTML.
         *
         * `NodeTest::Text` matches a CDATA section as well, and that is wanted
         * HERE: Lexbor's `:empty` ignores comments alone, so any other child -
         * a CDATA section included - makes the element non-empty. The
         * CDATA-excluding rule is `:lexbor-contains`'s, not this one. */
        PseudoClass::Empty => build::fold(
            b,
            Op::And,
            [NodeTest::ANY, NodeTest::Text, NodeTest::Pi(None)]
                .into_iter()
                .map(|kind| not_axis(b, Axis::Child, kind)),
            ":empty",
        ),
        /* The document element, not merely a parentless one - see [`root_test`]. */
        PseudoClass::Root => root_test(b),
        /* Added to `PseudoClass` for `lexbor::selector_port` (HTML matching);
         * the XML lowering doesn't implement any of them, same as before. */
        PseudoClass::AnyLink
        | PseudoClass::Link
        | PseudoClass::Blank
        | PseudoClass::Checked
        | PseudoClass::Disabled
        | PseudoClass::Enabled
        | PseudoClass::Optional
        | PseudoClass::Required
        | PseudoClass::ReadOnly
        | PseudoClass::ReadWrite
        | PseudoClass::Active
        | PseudoClass::Focus
        | PseudoClass::Hover
        | PseudoClass::Other => Err(b.fail(ErrorKind::Syntax, "unsupported CSS pseudo-class")),
    }
}

/// OR of the compound self-tests over each comma-argument of a selector list,
/// for `:is` / `:where` / `:not` - and, over a whole selector, for `matches?`.
pub(crate) fn selector_list_selftest(b: &Build, lists: Lists<'_>) -> Built {
    /* Lexbor rejects an empty list (`:is()`) before it gets here; answering it
     * anyway keeps every failure reported. */
    build::fold(
        b,
        Op::Or,
        lists.map(|g| complex_selftest(b, g.first())),
        "empty CSS selector list",
    )
}

/// `child::text()[is-text()][pred]` - the element's direct child TEXT nodes
/// satisfying `pred`, which is consumed.
///
/// In predicate position a non-empty node-set is truthy, so this reads "some
/// direct child text node matches" - exactly how Lexbor's `:lexbor-contains`
/// matcher scans, which looks at immediate child TEXT nodes only and not at the
/// deep string value. Matching that is what keeps the XML path's answer equal to
/// the HTML one.
///
/// The `is-text()` filter drops CDATA sections, which XPath's `text()` matches
/// but Lexbor's matcher does not: it scans `LXB_DOM_NODE_TYPE_TEXT` alone, so an
/// XML `:lexbor-contains` must not see a CDATA section either.
fn child_text_pred(b: &Build, pred: Built) -> Built {
    let pred = pred?;
    let mut step = Step::new(Axis::Child, NodeTest::Text);
    let is_text = build::call0(b, FN_IS_TEXT);
    build::push(b, &mut step.predicates, is_text?)?;
    build::push(b, &mut step.predicates, pred)?;
    build::single_step_path(b, step)
}

/// The functional pseudo-classes: `:nth-*(an+b)`, `:not()`, `:is()`/`:where()`,
/// `:has()`, `:lexbor-contains()`.
fn lower_pseudo_func(b: &Build, arg: FunctionArg<'_>) -> Built {
    match arg {
        FunctionArg::Nth {
            from_end,
            of_type,
            anb,
        } => {
            let Some(anb) = anb else {
                return Err(b.fail(ErrorKind::Syntax, "malformed :nth-*()"));
            };
            if anb.of {
                return Err(b.fail(ErrorKind::Syntax, ":nth-*(... of S) is not supported"));
            }
            let axis = if from_end {
                Axis::FollowingSibling
            } else {
                Axis::PrecedingSibling
            };
            let set = if of_type {
                Siblings::SameType
            } else {
                Siblings::All
            };
            nth(b, axis, set, anb)
        }

        FunctionArg::Selectors { pseudo, lists } => match pseudo {
            ListPseudo::Not => build::call1(b, b"not", || selector_list_selftest(b, lists)),
            ListPseudo::Is | ListPseudo::Where => selector_list_selftest(b, lists),
            /* OR of relative descendant/child paths; truthy when any matches.
             * Relative to self, so a leading >, + or ~ is honoured. */
            ListPseudo::Has => build::fold(
                b,
                Op::Or,
                lists.map(|g| complex(b, g.first(), true)),
                "empty CSS selector list",
            ),
        },

        FunctionArg::Contains(c) => {
            let Some(c) = c else {
                return Err(b.fail(ErrorKind::Syntax, "malformed :lexbor-contains()"));
            };
            let needle = c.needle;

            if !c.insensitive {
                let dot = build::step_path(b, Axis::SelfAxis, NodeTest::Node); /* "." */
                return child_text_pred(
                    b,
                    build::call2(b, b"contains", || dot, || build::literal(b, needle)),
                );
            }

            /* ASCII case-insensitive: fold both sides with translate(). The
             * flag is ASCII-only, which is what Lexbor's matcher does. */
            const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
            const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
            let Some(mut low) = crate::falloc::try_vec_with_capacity::<u8>(needle.len()) else {
                return Err(b.oom());
            };
            low.extend(needle.iter().map(|&ch| ch.to_ascii_lowercase()));

            let folded = build::call3(
                b,
                b"translate",
                || build::step_path(b, Axis::SelfAxis, NodeTest::Node),
                || build::literal(b, UPPER),
                || build::literal(b, LOWER),
            );
            child_text_pred(
                b,
                build::call2(b, b"contains", || folded, || build::literal(b, &low)),
            )
        }

        FunctionArg::Other => {
            Err(b.fail(ErrorKind::Syntax, "unsupported functional CSS pseudo-class"))
        }
    }
}

/// Push a predicate, freeing it if the list cannot grow. An `Err` predicate
/// means the builder that made it already failed.
fn push_pred(b: &Build, preds: &mut Vec<Expr>, p: Built) -> Result<(), Reported> {
    build::push(b, preds, p?)
}

/// Fold one simple selector into the current step: a type sets the node test,
/// everything else appends a predicate.
fn fold_simple(
    b: &Build,
    s: Selector<'_>,
    step: &mut Step,
    preds: &mut Vec<Expr>,
) -> Result<(), Reported> {
    match s.simple() {
        Simple::Universal => lower_universal(b, s, step, preds),
        Simple::Type => lower_type(b, s, step, preds),

        /* #id -> @id = 'id' */
        Simple::Id => {
            let lit = build::literal(b, s.name());
            push_pred(
                b,
                preds,
                build::binop(b, Op::Eq, || build::attr(b, b"id"), || lit),
            )
        }

        /* .class -> a token match on @class */
        Simple::Class => push_pred(b, preds, build::token_match(b, None, b"class", s.name())),

        Simple::Attribute(at) => push_pred(b, preds, lower_attribute(b, s, at)),
        Simple::PseudoClass(pc) => push_pred(b, preds, lower_pseudo_simple(b, pc)),
        Simple::PseudoClassFunction(arg) => push_pred(b, preds, lower_pseudo_func(b, arg)),

        Simple::PseudoElement => {
            Err(b.fail(ErrorKind::Syntax, "CSS pseudo-elements are not selectable"))
        }
        Simple::Other => Err(b.fail(ErrorKind::Syntax, "unsupported CSS selector component")),
    }
}

/* ------------------------------------------------------------------ *
 * compounds and chains                                               *
 * ------------------------------------------------------------------ */

/// The axis a combinator walks, forward (from the left compound to the right)
/// or in `reverse` (from the right back to the left).
///
/// `+` is not here: it takes two steps, which the callers emit. `Close` only
/// heads a chain's first compound - inside a chain it continues a compound
/// instead of starting one - and reads as whitespace there. Anything else Lexbor
/// parses (the column combinator `||`) is refused, never read as a descendant.
fn combinator_axis(b: &Build, c: Combinator, reverse: bool) -> Result<Axis, Reported> {
    Ok(match (c, reverse) {
        (Combinator::Descendant | Combinator::Close, false) => Axis::Descendant,
        (Combinator::Descendant | Combinator::Close, true) => Axis::Ancestor,
        (Combinator::Child, false) => Axis::Child,
        (Combinator::Child, true) => Axis::Parent,
        (Combinator::SubsequentSibling, false) => Axis::FollowingSibling,
        (Combinator::SubsequentSibling, true) => Axis::PrecedingSibling,
        (Combinator::NextSibling | Combinator::Other, _) => {
            return Err(b.fail(ErrorKind::Syntax, "unsupported CSS combinator"));
        }
    })
}

/// Build one step for the compound `[first ..= last]` and append it.
fn emit_compound_step(
    b: &Build,
    steps: &mut Vec<Step>,
    axis: Axis,
    comp: Compound<'_>,
) -> Result<(), Reported> {
    /* A type selector overrides the wildcard test. */
    let mut step = Step::new(axis, NodeTest::ANY);
    let mut preds = Vec::new();

    let mut cur = Some(comp.first);
    while let Some(s) = cur {
        fold_simple(b, s, &mut step, &mut preds)?;
        if s == comp.last {
            break;
        }
        cur = s.next();
    }

    step.predicates = preds;
    build::push(b, steps, step)
}

/// A compound - a CLOSE-linked run of simple selectors - and the combinator
/// connecting it to its left neighbour.
#[derive(Clone, Copy)]
struct Compound<'p> {
    first: Selector<'p>,
    last: Selector<'p>,
    comb: Combinator,
}

/// Walk a chain compound by compound, left to right.
///
/// The single splitter, shared by [`complex`] (forward) and [`complex_selftest`]
/// (right to left), so the boundary rule and the complexity cap the callers
/// apply live in one place rather than being written twice and drifting.
struct Compounds<'p> {
    cursor: Option<Selector<'p>>,
}

impl<'p> Iterator for Compounds<'p> {
    type Item = Compound<'p>;

    fn next(&mut self) -> Option<Compound<'p>> {
        let start = self.cursor?;
        /* A CLOSE combinator continues the compound. */
        let mut last = start;
        while let Some(nxt) = last.next().filter(|n| n.combinator() == Combinator::Close) {
            last = nxt;
        }
        self.cursor = last.next();
        Some(Compound {
            first: start,
            last,
            comb: start.combinator(),
        })
    }
}

/// `axis::*[1]` - the immediately adjacent sibling in either direction.
fn emit_adjacent_sibling(b: &Build, steps: &mut Vec<Step>, axis: Axis) -> Result<(), Reported> {
    let mut st = Step::new(axis, NodeTest::ANY);
    let p = build::num(b, 1.0)?;
    build::push(b, &mut st.predicates, p)?;
    build::push(b, steps, st)
}

/// Lower one complex selector (a chain) into a relative PATH node.
///
/// `relative_first` makes the FIRST compound honour its own combinator rather
/// than being forced to a descendant - which is what `:has(> a)`, `:has(+ a)`
/// and `:has(~ a)` need, since there the combinator is relative to self.
pub(crate) fn complex(b: &Build, first: Option<Selector<'_>>, relative_first: bool) -> Built {
    let _nesting = b.enter_selector_nesting()?;
    let mut steps = Vec::new();

    for (nc, comp) in (Compounds { cursor: first }).enumerate() {
        if nc >= MAX_COMPOUNDS {
            return Err(b.fail(ErrorKind::Limit, "CSS selector too complex"));
        }
        /* The first compound of a top-level query is a DESCENDANT of the
         * context node whatever it carries, which is what makes `css("p")` find
         * every `p` below the receiver rather than only its children. */
        if nc == 0 && !relative_first {
            emit_compound_step(b, &mut steps, Axis::Descendant, comp)?;
        } else if comp.comb == Combinator::NextSibling {
            /* `a + b` -> following-sibling::*[1] / self::b, two steps: XPath has
             * no adjacent-sibling axis, so "the next sibling" is the first one
             * on the following-sibling axis. */
            emit_adjacent_sibling(b, &mut steps, Axis::FollowingSibling)?;
            emit_compound_step(b, &mut steps, Axis::SelfAxis, comp)?;
        } else {
            let axis = combinator_axis(b, comp.comb, false)?;
            emit_compound_step(b, &mut steps, axis, comp)?;
        }
    }

    build::path(b, steps)
}

/// A boolean self-test for a (possibly multi-compound) complex selector, for
/// `:is()` / `:where()` / `:not()`.
///
/// Combinators become the reverse axes, so the path runs from the subject back
/// to its context:
///
/// - `a b`  -> `self::b/ancestor::a`
/// - `a > b` -> `self::b/parent::a`
/// - `a + b` -> `self::b/preceding-sibling::*[1]/self::a`
/// - `a ~ b` -> `self::b/preceding-sibling::a`
///
/// The path is non-empty - hence truthy - exactly when self matches.
pub(crate) fn complex_selftest(b: &Build, first: Option<Selector<'_>>) -> Built {
    let _nesting = b.enter_selector_nesting()?;
    /* The compounds go on the heap, not in a `[_; MAX_COMPOUNDS]` stack array:
     * that array was ~1.5 KiB per frame, and nesting selector lists (each
     * `:not` argument) put one on every level, so a few hundred levels ran the
     * native stack out before any depth check could fire. */
    let mut comps: Vec<Compound<'_>> = match crate::falloc::try_vec_with_capacity(MAX_COMPOUNDS) {
        Some(v) => v,
        None => return Err(b.oom()),
    };
    for comp in (Compounds { cursor: first }) {
        if comps.len() >= MAX_COMPOUNDS {
            return Err(b.fail(ErrorKind::Limit, "CSS selector too complex"));
        }
        build::push(b, &mut comps, comp)?;
    }

    /* Right to left: the subject first, then each compound to its left, joined
     * by the combinator that sits on its right neighbour. */
    let mut leftward = comps.iter().rev().copied();
    let Some(subject) = leftward.next() else {
        return Err(b.fail(ErrorKind::Syntax, "empty CSS selector"));
    };

    let mut steps = Vec::new();
    emit_compound_step(b, &mut steps, Axis::SelfAxis, subject)?;

    let mut right = subject;
    for left in leftward {
        if right.comb == Combinator::NextSibling {
            /* Reverse adjacent: the immediately preceding sibling must match. */
            emit_adjacent_sibling(b, &mut steps, Axis::PrecedingSibling)?;
            emit_compound_step(b, &mut steps, Axis::SelfAxis, left)?;
        } else {
            let axis = combinator_axis(b, right.comb, true)?;
            emit_compound_step(b, &mut steps, axis, left)?;
        }
        right = left;
    }

    build::path(b, steps)
}
