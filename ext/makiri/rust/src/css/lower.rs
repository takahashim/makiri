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
use crate::lexbor::css_parser::{
    AttrMatch, Attribute, Combinator, FunctionArg, ListPseudo, Lists, Nth, PseudoClass, Selector,
    Simple,
};
use crate::xpath::ast::{Axis, Expr, NodeTest, Op, Step, TestKind};
use crate::xpath::msg::{Reported, XP_ERR_LIMIT, XP_ERR_SYNTAX};

/// The internal of-type position functions, whose names carry a leading \x01 so
/// no user expression can name them.
use crate::xpath::funcs::{FN_OF_TYPE_POS, FN_OF_TYPE_POS_LAST};

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
        step.test.kind = TestKind::Wildcard;
        let ln = build::call(b, b"local-name", []);
        let lit = build::literal(b, name);
        return push_pred(b, preds, build::binop(b, Op::Eq, ln, lit));
    }

    step.test.kind = TestKind::Name;
    step.test.local = Some(build::copy_text(b, name)?);

    match ns {
        /* `p|el` */
        Some(p) if !p.is_empty() => {
            step.test.prefix = Some(build::copy_text(b, p)?);
            Ok(())
        }
        /* `|el`: an explicit no-namespace, so leave the prefix unset. */
        Some(_) => Ok(()),
        /* A bare `el`. With a document default namespace in scope it binds to
         * the synthetic prefix, which is Nokogiri's behaviour. */
        None => match b.default_prefix() {
            Some(dp) => {
                step.test.prefix = Some(build::copy_text(b, dp)?);
                Ok(())
            }
            None => Ok(()),
        },
    }
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
    step.test.kind = TestKind::Wildcard;
    match s.ns() {
        None | Some(b"*") => Ok(()),
        Some(b"") => {
            let uri = build::call(b, b"namespace-uri", []);
            let lit = build::literal(b, b"");
            push_pred(b, preds, build::binop(b, Op::Eq, uri, lit))
        }
        Some(p) => {
            step.test.prefix = Some(build::copy_text(b, p)?);
            Ok(())
        }
    }
}

/// `[name op value]` as an expression.
fn lower_attribute(b: &Build, s: Selector<'_>, at: Attribute<'_>) -> Built {
    let name = s.name();

    if at.case_modifier {
        return Err(b.fail(
            XP_ERR_SYNTAX,
            c"CSS attribute case modifier ([a=v i]) is not supported",
        ));
    }

    /* The attribute namespace: NULL is a bare name (no namespace, the common
     * case), `*` is any (unsupported), anything else is a prefix. An unprefixed
     * CSS attribute selector matches the no-namespace attribute, per CSS and
     * XPath alike. */
    let prefix = match s.ns() {
        Some(p) if p == b"*" => {
            return Err(b.fail(
                XP_ERR_SYNTAX,
                c"any-namespace attribute selectors ([*|a]) are not supported",
            ));
        }
        other => other, /* a zero-length one (|a) means no namespace */
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
            value.is_empty() || value.iter().any(|&c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c))
        }
        _ => false,
    };
    if never {
        return build::call(b, b"false", []);
    }

    match at.op {
        /* [a=v] -> @a = 'v' */
        AttrMatch::Equal => build::binop(
            b,
            Op::Eq,
            build::attr_ns(b, prefix, name),
            build::literal(b, value),
        ),
        /* [a~=v] -> whitespace-separated token match */
        AttrMatch::Include => build::token_match(b, prefix, name, value),
        /* [a^=v] -> starts-with(@a, 'v') */
        AttrMatch::Prefix => build::call2(
            b,
            b"starts-with",
            build::attr_ns(b, prefix, name),
            build::literal(b, value),
        ),
        /* [a*=v] -> contains(@a, 'v') */
        AttrMatch::Substring => build::call2(
            b,
            b"contains",
            build::attr_ns(b, prefix, name),
            build::literal(b, value),
        ),
        /* [a$=v] -> substring(@a, string-length(@a) - len + 1) = 'v', where len
         * counts CHARACTERS: string-length and substring do, so a byte count
         * missed any non-ASCII suffix ([d$="é"]). */
        AttrMatch::Suffix => {
            let slen = build::call1(b, b"string-length", build::attr_ns(b, prefix, name));
            let chars = value.iter().filter(|&&c| c & 0xC0 != 0x80).count();
            let start = build::binop(
                b,
                Op::Add,
                build::binop(b, Op::Sub, slen, build::num(b, chars as f64)),
                build::num(b, 1.0),
            );
            let sub = build::call2(b, b"substring", build::attr_ns(b, prefix, name), start);
            build::binop(b, Op::Eq, sub, build::literal(b, value))
        }
        /* [a|=v] -> @a = 'v' or starts-with(@a, 'v-') */
        AttrMatch::Dash => {
            let eq = build::binop(
                b,
                Op::Eq,
                build::attr_ns(b, prefix, name),
                build::literal(b, value),
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
                build::attr_ns(b, prefix, name),
                build::literal(b, &dashed),
            );
            build::binop(b, Op::Or, eq, pre)
        }
        AttrMatch::Other => Err(b.fail(XP_ERR_SYNTAX, c"unsupported CSS attribute operator")),
    }
}

/// `not(axis::nt)` - "nothing on that axis".
fn not_axis(b: &Build, axis: Axis, nt: TestKind) -> Built {
    build::call1(b, b"not", build::step_path(b, axis, nt))
}

/// The siblings a structural pseudo-class counts among.
#[derive(Clone, Copy)]
enum Siblings<'t> {
    /// Every element sibling: the `-child` family.
    All,
    /// Siblings named like the compound's type selector: a typed of-type
    /// (`a:first-of-type`).
    Named(&'t NodeTest),
    /// Siblings with the element's OWN expanded name: an untyped of-type
    /// (`:first-of-type`). Pure XPath 1.0 cannot say "same name as self", so
    /// this is an internal function compared at eval time.
    SameType,
}

impl<'t> Siblings<'t> {
    /// The of-type set for a compound whose node test so far is `test`.
    fn of_type(test: &'t NodeTest) -> Self {
        if test.kind == TestKind::Name {
            Siblings::Named(test)
        } else {
            Siblings::SameType
        }
    }

    /// `axis::` restricted to this set, as a relative path - or None for
    /// [`Siblings::SameType`], which no path can express.
    fn path(self, b: &Build, axis: Axis) -> Option<Built> {
        match self {
            Siblings::All => Some(build::step_path(b, axis, TestKind::Wildcard)),
            Siblings::Named(t) => Some(build::named_step_path(
                b,
                axis,
                t.prefix.as_deref(),
                t.local.as_deref().unwrap_or(&[]),
            )),
            Siblings::SameType => None,
        }
    }
}

/// The internal of-type position call: 1-based among same-type siblings,
/// counting from the start when `axis` looks back, from the end otherwise.
fn of_type_pos(b: &Build, axis: Axis) -> Built {
    let name = if axis == Axis::PrecedingSibling {
        FN_OF_TYPE_POS
    } else {
        FN_OF_TYPE_POS_LAST
    };
    build::call(b, name, [])
}

/// The 1-based position among `set`, counted along `axis`:
/// `count(axis::test) + 1`.
fn position(b: &Build, axis: Axis, set: Siblings<'_>) -> Built {
    match set.path(b, axis) {
        Some(path) => build::binop(
            b,
            Op::Add,
            build::call1(b, b"count", path),
            build::num(b, 1.0),
        ),
        None => of_type_pos(b, axis),
    }
}

/// "No sibling of `set` along `axis`" - first (looking back) or last (looking
/// forward) among them.
fn none_along(b: &Build, axis: Axis, set: Siblings<'_>) -> Built {
    match set.path(b, axis) {
        Some(path) => build::call1(b, b"not", path),
        None => build::binop(b, Op::Eq, of_type_pos(b, axis), build::num(b, 1.0)),
    }
}

/// Both first and last among `set`: the `only-` family.
fn only(b: &Build, set: Siblings<'_>) -> Built {
    build::binop(
        b,
        Op::And,
        none_along(b, Axis::PrecedingSibling, set),
        none_along(b, Axis::FollowingSibling, set),
    )
}

/// The `:nth-*(an+b)` match condition over the position among `set` along
/// `axis`.
fn nth(b: &Build, axis: Axis, set: Siblings<'_>, anb: Nth) -> Built {
    /* `c_long` from Lexbor's `lxb_css_syntax_anb_t` - 64-bit on LP64, 32-bit on
     * LLP64 - so the `as f64` below is a real conversion on either. */
    let (a, bb) = (anb.a as f64, anb.b as f64);
    if anb.a == 0 {
        /* position = b */
        return build::binop(b, Op::Eq, position(b, axis, set), build::num(b, bb));
    }
    /* (pos - b) mod a == 0  AND  (pos - b) div a >= 0 - the second rules out a
     * negative index, which the modulo alone would accept. */
    let d1 = build::binop(b, Op::Sub, position(b, axis, set), build::num(b, bb));
    let modz = build::binop(
        b,
        Op::Eq,
        build::binop(b, Op::Mod, d1, build::num(b, a)),
        build::num(b, 0.0),
    );
    let d2 = build::binop(b, Op::Sub, position(b, axis, set), build::num(b, bb));
    let qge = build::binop(
        b,
        Op::Ge,
        build::binop(b, Op::Div, d2, build::num(b, a)),
        build::num(b, 0.0),
    );
    build::binop(b, Op::And, modz, qge)
}

/// The non-functional structural pseudo-classes. `test` - the compound's node
/// test so far - supplies the element name for the of-type family.
fn lower_pseudo_simple(b: &Build, pc: PseudoClass, test: &NodeTest) -> Built {
    let of_type = Siblings::of_type(test);
    match pc {
        PseudoClass::FirstChild => none_along(b, Axis::PrecedingSibling, Siblings::All),
        PseudoClass::LastChild => none_along(b, Axis::FollowingSibling, Siblings::All),
        PseudoClass::OnlyChild => only(b, Siblings::All),
        PseudoClass::FirstOfType => none_along(b, Axis::PrecedingSibling, of_type),
        PseudoClass::LastOfType => none_along(b, Axis::FollowingSibling, of_type),
        PseudoClass::OnlyOfType => only(b, of_type),
        /* not(node()) */
        /* No child but comments, as the HTML matcher (Lexbor) has it: an
         * element, text or processing instruction makes it non-empty. `not(node())`
         * counted a comment too, so <e><!--c--></e> was empty only in HTML. */
        PseudoClass::Empty => build::fold(
            b,
            Op::And,
            [TestKind::Wildcard, TestKind::Text, TestKind::Pi]
                .into_iter()
                .map(|kind| not_axis(b, Axis::Child, kind)),
            c":empty",
        ),
        /* not(parent::*) */
        PseudoClass::Root => not_axis(b, Axis::Parent, TestKind::Wildcard),
        PseudoClass::Other => Err(b.fail(XP_ERR_SYNTAX, c"unsupported CSS pseudo-class")),
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
        c"empty CSS selector list",
    )
}

/// `child::text()[pred]` - the element's direct child text nodes satisfying
/// `pred`, which is consumed.
///
/// In predicate position a non-empty node-set is truthy, so this reads "some
/// direct child text node matches" - exactly how Lexbor's `:lexbor-contains`
/// matcher scans, which looks at immediate child TEXT nodes only and not at the
/// deep string value. Matching that is what keeps the XML path's answer equal to
/// the HTML one.
fn child_text_pred(b: &Build, pred: Built) -> Built {
    let pred = pred?;
    let mut step = Step::new(Axis::Child, TestKind::Text);
    build::push(b, &mut step.predicates, pred)?;
    build::single_step_path(b, step)
}

/// The functional pseudo-classes: `:nth-*(an+b)`, `:not()`, `:is()`/`:where()`,
/// `:has()`, `:lexbor-contains()`.
fn lower_pseudo_func(b: &Build, arg: FunctionArg<'_>, test: &NodeTest) -> Built {
    match arg {
        FunctionArg::Nth {
            from_end,
            of_type,
            anb,
        } => {
            let Some(anb) = anb else {
                return Err(b.fail(XP_ERR_SYNTAX, c"malformed :nth-*()"));
            };
            if anb.of {
                return Err(b.fail(XP_ERR_SYNTAX, c":nth-*(... of S) is not supported"));
            }
            let axis = if from_end {
                Axis::FollowingSibling
            } else {
                Axis::PrecedingSibling
            };
            let set = if of_type {
                Siblings::of_type(test)
            } else {
                Siblings::All
            };
            nth(b, axis, set, anb)
        }

        FunctionArg::Selectors { pseudo, lists } => match pseudo {
            ListPseudo::Not => build::call1(b, b"not", selector_list_selftest(b, lists)),
            ListPseudo::Is | ListPseudo::Where => selector_list_selftest(b, lists),
            /* OR of relative descendant/child paths; truthy when any matches.
             * Relative to self, so a leading >, + or ~ is honoured. */
            ListPseudo::Has => build::fold(
                b,
                Op::Or,
                lists.map(|g| complex(b, g.first(), true)),
                c"empty CSS selector list",
            ),
        },

        FunctionArg::Contains(c) => {
            let Some(c) = c else {
                return Err(b.fail(XP_ERR_SYNTAX, c"malformed :lexbor-contains()"));
            };
            let needle = c.needle;

            if !c.insensitive {
                let dot = build::step_path(b, Axis::SelfAxis, TestKind::Node); /* "." */
                return child_text_pred(
                    b,
                    build::call2(b, b"contains", dot, build::literal(b, needle)),
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

            let folded = build::call(
                b,
                b"translate",
                [
                    build::step_path(b, Axis::SelfAxis, TestKind::Node),
                    build::literal(b, UPPER),
                    build::literal(b, LOWER),
                ],
            );
            child_text_pred(
                b,
                build::call2(b, b"contains", folded, build::literal(b, &low)),
            )
        }

        FunctionArg::Other => {
            Err(b.fail(XP_ERR_SYNTAX, c"unsupported functional CSS pseudo-class"))
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
                build::binop(b, Op::Eq, build::attr(b, b"id"), lit),
            )
        }

        /* .class -> a token match on @class */
        Simple::Class => push_pred(b, preds, build::token_match(b, None, b"class", s.name())),

        Simple::Attribute(at) => push_pred(b, preds, lower_attribute(b, s, at)),
        Simple::PseudoClass(pc) => push_pred(b, preds, lower_pseudo_simple(b, pc, &step.test)),
        Simple::PseudoClassFunction(arg) => {
            push_pred(b, preds, lower_pseudo_func(b, arg, &step.test))
        }

        Simple::PseudoElement => {
            Err(b.fail(XP_ERR_SYNTAX, c"CSS pseudo-elements are not selectable"))
        }
        Simple::Other => Err(b.fail(XP_ERR_SYNTAX, c"unsupported CSS selector component")),
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
            return Err(b.fail(XP_ERR_SYNTAX, c"unsupported CSS combinator"));
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
    let mut step = Step::new(axis, TestKind::Wildcard);
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
    let mut st = Step::new(axis, TestKind::Wildcard);
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
    let mut steps = Vec::new();

    for (nc, comp) in (Compounds { cursor: first }).enumerate() {
        if nc >= MAX_COMPOUNDS {
            return Err(b.fail(XP_ERR_LIMIT, c"CSS selector too complex"));
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
    let mut comps: [Option<Compound<'_>>; MAX_COMPOUNDS] = [None; MAX_COMPOUNDS];
    let mut nc = 0usize;
    for comp in (Compounds { cursor: first }) {
        if nc >= MAX_COMPOUNDS {
            return Err(b.fail(XP_ERR_LIMIT, c"CSS selector too complex"));
        }
        comps[nc] = Some(comp);
        nc += 1;
    }

    /* Right to left: the subject first, then each compound to its left, joined
     * by the combinator that sits on its right neighbour. */
    let mut leftward = comps[..nc].iter().rev().flatten();
    let Some(&subject) = leftward.next() else {
        return Err(b.fail(XP_ERR_SYNTAX, c"empty CSS selector"));
    };

    let mut steps = Vec::new();
    emit_compound_step(b, &mut steps, Axis::SelfAxis, subject)?;

    let mut right = subject;
    for &left in leftward {
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
