//! The lowering itself: a Lexbor selector chain becomes XPath steps and
//! predicates.
//!
//! The shape follows the C exactly, because the mapping from CSS to XPath is the
//! interesting content and it is already worked out there:
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
//! The parsed selectors are read only through `parser`'s typed views, so this
//! module holds no pointer into Lexbor's arena and forbids unsafe.

#![forbid(unsafe_code)]

use super::build::{self, Built};
use super::parser::{comb, k, m, pc, pf, FunctionArg, Lists, Selector};
use super::{Build, ERR_LIMIT, ERR_SYNTAX, MAX_COMPOUNDS};
use crate::xpath::ast::{Axis, Expr, ExprKind, NodeTest, Op, Path, Step, TestKind};
use crate::xpath::msg::Reported;

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

    if s.kind() == k::ANY {
        /* `*` or `ns|*` */
        step.test.kind = TestKind::Wildcard;
        return Ok(());
    }

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

/// `[name op value]` as an expression.
fn lower_attribute(b: &Build, s: Selector<'_>) -> Built {
    let name = s.name();
    let Some(at) = s.attribute() else {
        return Err(b.fail(ERR_SYNTAX, c"unsupported CSS selector component"));
    };

    if at.modifier == m::MOD_I || at.modifier == m::MOD_S {
        return Err(b.fail(
            ERR_SYNTAX,
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
                ERR_SYNTAX,
                c"any-namespace attribute selectors ([*|a]) are not supported",
            ));
        }
        other => other, /* a zero-length one (|a) means no namespace */
    };

    let Some(value) = at.value else {
        /* `[name]` - existence */
        return build::attr_ns(b, prefix, name);
    };

    match at.match_ {
        /* [a=v] -> @a = 'v' */
        m::EQUAL => build::binop(
            b,
            Op::Eq,
            build::attr_ns(b, prefix, name),
            build::literal(b, value),
        ),
        /* [a~=v] -> whitespace-separated token match */
        m::INCLUDE => build::token_match(b, prefix, name, value),
        /* [a^=v] -> starts-with(@a, 'v') */
        m::PREFIX => build::call2(
            b,
            b"starts-with",
            build::attr_ns(b, prefix, name),
            build::literal(b, value),
        ),
        /* [a*=v] -> contains(@a, 'v') */
        m::SUBSTRING => build::call2(
            b,
            b"contains",
            build::attr_ns(b, prefix, name),
            build::literal(b, value),
        ),
        /* [a$=v] -> substring(@a, string-length(@a) - len + 1) = 'v' */
        m::SUFFIX => {
            let slen = build::call1(b, b"string-length", build::attr_ns(b, prefix, name));
            let start = build::binop(
                b,
                Op::Add,
                build::binop(b, Op::Sub, slen, build::num(b, value.len() as f64)),
                build::num(b, 1.0),
            );
            let sub = build::call2(b, b"substring", build::attr_ns(b, prefix, name), start);
            build::binop(b, Op::Eq, sub, build::literal(b, value))
        }
        /* [a|=v] -> @a = 'v' or starts-with(@a, 'v-') */
        m::DASH => {
            let eq = build::binop(
                b,
                Op::Eq,
                build::attr_ns(b, prefix, name),
                build::literal(b, value),
            );
            let mut dashed = match crate::falloc::try_vec_with_capacity::<u8>(value.len() + 1) {
                Some(v) => v,
                None => {
                    return Err(b.oom());
                }
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
        _ => Err(b.fail(ERR_SYNTAX, c"unsupported CSS attribute operator")),
    }
}

/// `not(axis::*)` - "no sibling or child on that axis".
fn not_axis(b: &Build, axis: Axis, nt: TestKind) -> Built {
    build::call1(b, b"not", build::step_path(b, axis, nt, None))
}

/// `not([prefix:]name on axis)` - "no same-named sibling on that axis".
fn not_named_axis(b: &Build, axis: Axis, test: &NodeTest) -> Built {
    build::call1(
        b,
        b"not",
        build::named_step_path(
            b,
            axis,
            test.prefix.as_deref(),
            test.local.as_deref().unwrap_or(&[]),
        ),
    )
}

/// `count(axis::test) + 1` - the 1-based position among matched siblings.
fn pos(b: &Build, axis: Axis, named: Option<&NodeTest>) -> Built {
    let path = match named {
        None => build::step_path(b, axis, TestKind::Wildcard, None),
        Some(t) => build::named_step_path(
            b,
            axis,
            t.prefix.as_deref(),
            t.local.as_deref().unwrap_or(&[]),
        ),
    };
    build::binop(
        b,
        Op::Add,
        build::call1(b, b"count", path),
        build::num(b, 1.0),
    )
}

/// The internal of-type position call: 1-based among same-type siblings,
/// counting forward (from the start) or backward.
///
/// This exists because an UNTYPED of-type compares the element's own expanded
/// name against its siblings', which pure XPath 1.0 cannot express - there is no
/// way to say "same name as self".
fn of_type_pos(b: &Build, forward: bool) -> Built {
    let name = if forward {
        FN_OF_TYPE_POS
    } else {
        FN_OF_TYPE_POS_LAST
    };
    build::call(b, name, [])
}

/// The 1-based position expression for `:nth-*`.
fn pos_expr(b: &Build, axis: Axis, named: Option<&NodeTest>, oftype_untyped: bool) -> Built {
    if oftype_untyped {
        return of_type_pos(b, axis == Axis::PrecedingSibling);
    }
    pos(b, axis, named)
}

/// The `:nth-*(an+b)` match condition over the position expression on `axis`.
fn nth(
    b: &Build,
    axis: Axis,
    named: Option<&NodeTest>,
    oftype_untyped: bool,
    /* `c_long`, not i64: these come straight from Lexbor's
     * `lxb_css_syntax_anb_t`, whose fields are C `long` - 64-bit on LP64 and
     * 32-bit on Windows's LLP64. Matching the generated type keeps the call site
     * cast-free on every platform, and the `as f64` uses below are a real
     * conversion either way, so no lint fires on one platform or the other. */
    a: core::ffi::c_long,
    bb: core::ffi::c_long,
) -> Built {
    if a == 0 {
        /* position = b */
        return build::binop(
            b,
            Op::Eq,
            pos_expr(b, axis, named, oftype_untyped),
            build::num(b, bb as f64),
        );
    }
    /* (pos - b) mod a == 0  AND  (pos - b) div a >= 0 - the second rules out a
     * negative index, which the modulo alone would accept. */
    let d1 = build::binop(
        b,
        Op::Sub,
        pos_expr(b, axis, named, oftype_untyped),
        build::num(b, bb as f64),
    );
    let modz = build::binop(
        b,
        Op::Eq,
        build::binop(b, Op::Mod, d1, build::num(b, a as f64)),
        build::num(b, 0.0),
    );
    let d2 = build::binop(
        b,
        Op::Sub,
        pos_expr(b, axis, named, oftype_untyped),
        build::num(b, bb as f64),
    );
    let qge = build::binop(
        b,
        Op::Ge,
        build::binop(b, Op::Div, d2, build::num(b, a as f64)),
        build::num(b, 0.0),
    );
    build::binop(b, Op::And, modz, qge)
}

/// The non-functional structural pseudo-classes. `test` - the compound's node
/// test so far - supplies the element name for the of-type family.
fn lower_pseudo_simple(b: &Build, s: Selector<'_>, test: &NodeTest) -> Built {
    let Some(pt) = s.pseudo_id() else {
        return Err(b.fail(ERR_SYNTAX, c"unsupported CSS pseudo-class"));
    };
    match pt {
        pc::FIRST_CHILD => not_axis(b, Axis::PrecedingSibling, TestKind::Wildcard),
        pc::LAST_CHILD => not_axis(b, Axis::FollowingSibling, TestKind::Wildcard),
        pc::ONLY_CHILD => build::binop(
            b,
            Op::And,
            not_axis(b, Axis::PrecedingSibling, TestKind::Wildcard),
            not_axis(b, Axis::FollowingSibling, TestKind::Wildcard),
        ),
        /* not(node()) */
        pc::EMPTY => not_axis(b, Axis::Child, TestKind::Node),
        /* not(parent::*) */
        pc::ROOT => not_axis(b, Axis::Parent, TestKind::Wildcard),

        pc::FIRST_OF_TYPE | pc::LAST_OF_TYPE | pc::ONLY_OF_TYPE => {
            /* Typed (`a:first-of-type`) becomes not(preceding-sibling::a);
             * untyped (`:first-of-type`) becomes of-type-pos() = 1, where the
             * type is the element's own expanded name, compared at eval time. */
            if test.kind != TestKind::Name {
                let first_is_one = |b: &Build, fwd: bool| {
                    build::binop(b, Op::Eq, of_type_pos(b, fwd), build::num(b, 1.0))
                };
                return match pt {
                    pc::FIRST_OF_TYPE => first_is_one(b, true),
                    pc::LAST_OF_TYPE => first_is_one(b, false),
                    _ => build::binop(b, Op::And, first_is_one(b, true), first_is_one(b, false)),
                };
            }
            match pt {
                pc::FIRST_OF_TYPE => not_named_axis(b, Axis::PrecedingSibling, test),
                pc::LAST_OF_TYPE => not_named_axis(b, Axis::FollowingSibling, test),
                _ => build::binop(
                    b,
                    Op::And,
                    not_named_axis(b, Axis::PrecedingSibling, test),
                    not_named_axis(b, Axis::FollowingSibling, test),
                ),
            }
        }

        _ => Err(b.fail(ERR_SYNTAX, c"unsupported CSS pseudo-class")),
    }
}

/// OR of the compound self-tests over each comma-argument of a selector list,
/// for `:is` / `:where` / `:not`.
fn selector_list_selftest(b: &Build, lists: Lists<'_>) -> Built {
    let mut acc: Option<Expr> = None;
    for g in lists {
        let one = complex_selftest(b, g.first())?;
        acc = Some(match acc {
            None => one,
            Some(lhs) => build::binop(b, Op::Or, Ok(lhs), Ok(one))?,
        });
    }
    /* Lexbor rejects an empty list (`:is()`) before it gets here; answering it
     * anyway keeps every failure reported. */
    acc.ok_or_else(|| b.fail(ERR_SYNTAX, c"empty CSS selector list"))
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
    build::charge(b)?;
    let mut step = Step::new(Axis::Child, TestKind::Text);
    build::push(b, &mut step.predicates, pred)?;
    let mut steps = Vec::new();
    build::push(b, &mut steps, step)?;
    build::expr(
        b,
        ExprKind::Path(Path {
            absolute: false,
            steps,
        }),
    )
}

/// The functional pseudo-classes: `:nth-*(an+b)`, `:not()`, `:is()`/`:where()`,
/// `:has()`, `:lexbor-contains()`.
fn lower_pseudo_func(b: &Build, s: Selector<'_>, test: &NodeTest) -> Built {
    let ty = s.pseudo_id().unwrap_or(0);

    match s.function_arg() {
        FunctionArg::Nth(anb) => {
            let Some(anb) = anb else {
                return Err(b.fail(ERR_SYNTAX, c"malformed :nth-*()"));
            };
            if anb.of {
                return Err(b.fail(ERR_SYNTAX, c":nth-*(... of S) is not supported"));
            }
            let last = ty == pf::NTH_LAST_CHILD || ty == pf::NTH_LAST_OF_TYPE;
            let of_type = ty == pf::NTH_OF_TYPE || ty == pf::NTH_LAST_OF_TYPE;
            let axis = if last {
                Axis::FollowingSibling
            } else {
                Axis::PrecedingSibling
            };

            /* Typed of-type counts same-name siblings through a literal name;
             * untyped compares the element's own expanded name at eval time. */
            let mut named = None;
            let mut untyped = false;
            if of_type {
                if test.kind == TestKind::Name {
                    named = Some(test);
                } else {
                    untyped = true;
                }
            }
            nth(b, axis, named, untyped, anb.a, anb.b)
        }

        FunctionArg::Selectors(lists) => match ty {
            pf::NOT => {
                let inner = selector_list_selftest(b, lists)?;
                build::call1(b, b"not", Ok(inner))
            }
            pf::HAS => {
                /* OR of relative descendant/child paths; truthy when any matches. */
                let mut acc: Option<Expr> = None;
                for g in lists {
                    /* Relative to self, so a leading >, + or ~ is honoured. */
                    let path = complex(b, g.first(), true)?;
                    acc = Some(match acc {
                        None => path,
                        Some(lhs) => build::binop(b, Op::Or, Ok(lhs), Ok(path))?,
                    });
                }
                acc.ok_or_else(|| b.fail(ERR_SYNTAX, c"empty CSS selector list"))
            }
            /* :is / :where */
            _ => selector_list_selftest(b, lists),
        },

        FunctionArg::Contains(c) => {
            let Some(c) = c else {
                return Err(b.fail(ERR_SYNTAX, c"malformed :lexbor-contains()"));
            };
            let needle = c.needle;

            if !c.insensitive {
                let dot = build::step_path(b, Axis::SelfAxis, TestKind::Node, None); /* "." */
                return child_text_pred(
                    b,
                    build::call2(b, b"contains", dot, build::literal(b, needle)),
                );
            }

            /* ASCII case-insensitive: fold both sides with translate(). The
             * flag is ASCII-only, which is what Lexbor's matcher does. */
            const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
            const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
            let mut low = match crate::falloc::try_vec_with_capacity::<u8>(needle.len()) {
                Some(v) => v,
                None => {
                    return Err(b.oom());
                }
            };
            low.extend(needle.iter().map(|&ch| ch.to_ascii_lowercase()));

            let folded = build::call(
                b,
                b"translate",
                [
                    build::step_path(b, Axis::SelfAxis, TestKind::Node, None),
                    build::literal(b, UPPER),
                    build::literal(b, LOWER),
                ],
            );
            child_text_pred(
                b,
                build::call2(b, b"contains", folded, build::literal(b, &low)),
            )
        }

        FunctionArg::Other => Err(b.fail(ERR_SYNTAX, c"unsupported functional CSS pseudo-class")),
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
    match s.kind() {
        k::ANY | k::ELEMENT => lower_type(b, s, step, preds),

        /* #id -> @id = 'id' */
        k::ID => {
            let lit = build::literal(b, s.name());
            push_pred(
                b,
                preds,
                build::binop(b, Op::Eq, build::attr(b, b"id"), lit),
            )
        }

        /* .class -> a token match on @class */
        k::CLASS => push_pred(b, preds, build::token_match(b, None, b"class", s.name())),

        k::ATTRIBUTE => push_pred(b, preds, lower_attribute(b, s)),
        k::PSEUDO_CLASS => push_pred(b, preds, lower_pseudo_simple(b, s, &step.test)),
        k::PSEUDO_CLASS_FUNCTION => push_pred(b, preds, lower_pseudo_func(b, s, &step.test)),

        k::PSEUDO_ELEMENT | k::PSEUDO_ELEMENT_FUNCTION => {
            Err(b.fail(ERR_SYNTAX, c"CSS pseudo-elements are not selectable"))
        }
        _ => Err(b.fail(ERR_SYNTAX, c"unsupported CSS selector component")),
    }
}

/* ------------------------------------------------------------------ *
 * compounds and chains                                               *
 * ------------------------------------------------------------------ */

/// The axis connecting a compound to its predecessor, from its combinator.
///
/// The first compound of a top-level query is a DESCENDANT of the context node
/// whatever it carries, which is what makes `css("p")` find every `p` below the
/// receiver rather than only its children.
fn axis_for_combinator(c: u32, is_first: bool) -> Axis {
    if is_first {
        return Axis::Descendant;
    }
    match c {
        comb::CHILD => Axis::Child,
        comb::FOLLOWING => Axis::FollowingSibling, /* ~ */
        _ => Axis::Descendant,
    }
}

/// The reverse of a forward combinator, for walking from the subject back to the
/// preceding compound. Adjacent (`+`) is handled separately, as two steps.
fn reverse_axis(c: u32) -> Axis {
    match c {
        comb::CHILD => Axis::Parent,
        comb::FOLLOWING => Axis::PrecedingSibling, /* ~ */
        _ => Axis::Ancestor,
    }
}

/// Build one step for the compound `[first ..= last]` and append it.
fn emit_compound_step(
    b: &Build,
    steps: &mut Vec<Step>,
    axis: Axis,
    first: Selector<'_>,
    last: Selector<'_>,
) -> Result<(), Reported> {
    /* A type selector overrides the wildcard test. */
    let mut step = Step::new(axis, TestKind::Wildcard);
    let mut preds = Vec::new();

    let mut cur = Some(first);
    while let Some(s) = cur {
        fold_simple(b, s, &mut step, &mut preds)?;
        if s == last {
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
    comb: u32,
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
        while let Some(nxt) = last.next().filter(|n| n.combinator() == comb::CLOSE) {
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

/// Lower one complex selector (a chain) into a relative PATH node.
///
/// `relative_first` makes the FIRST compound honour its own combinator rather
/// than being forced to a descendant - which is what `:has(> a)`, `:has(+ a)`
/// and `:has(~ a)` need, since there the combinator is relative to self.
pub(crate) fn complex(b: &Build, first: Option<Selector<'_>>, relative_first: bool) -> Built {
    let mut steps = Vec::new();

    for (nc, comp) in (Compounds { cursor: first }).enumerate() {
        if nc >= MAX_COMPOUNDS {
            return Err(b.fail(ERR_LIMIT, c"CSS selector too complex"));
        }
        let is_first = nc == 0 && !relative_first;

        if !is_first && comp.comb == comb::SIBLING {
            /* `a + b` -> following-sibling::*[1] / self::b, two steps: XPath has
             * no adjacent-sibling axis, so "the next sibling" is the first one
             * on the following-sibling axis. */
            emit_adjacent(b, &mut steps)?;
            emit_compound_step(b, &mut steps, Axis::SelfAxis, comp.first, comp.last)?;
        } else {
            let axis = axis_for_combinator(comp.comb, is_first);
            emit_compound_step(b, &mut steps, axis, comp.first, comp.last)?;
        }
    }

    finish_path(b, steps)
}

/// The `following-sibling::*[1]` step of an adjacent combinator.
fn emit_adjacent(b: &Build, steps: &mut Vec<Step>) -> Result<(), Reported> {
    emit_positional_sibling(b, steps, Axis::FollowingSibling)
}

/// `axis::*[1]` - the immediately adjacent sibling in either direction.
fn emit_positional_sibling(b: &Build, steps: &mut Vec<Step>, axis: Axis) -> Result<(), Reported> {
    let mut st = Step::new(axis, TestKind::Wildcard);
    let p = build::num(b, 1.0)?;
    build::push(b, &mut st.predicates, p)?;
    build::push(b, steps, st)
}

/// Wrap built steps in a relative PATH node, or free them on failure.
fn finish_path(b: &Build, steps: Vec<Step>) -> Built {
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
            return Err(b.fail(ERR_LIMIT, c"CSS selector too complex"));
        }
        comps[nc] = Some(comp);
        nc += 1;
    }

    /* Right to left: the subject first, then each compound to its left, joined
     * by the combinator that sits on its right neighbour. */
    let mut leftward = comps[..nc].iter().rev().flatten();
    let Some(&subject) = leftward.next() else {
        return Err(b.fail(ERR_SYNTAX, c"empty CSS selector"));
    };

    let mut steps = Vec::new();
    emit_compound_step(b, &mut steps, Axis::SelfAxis, subject.first, subject.last)?;

    let mut right = subject;
    for &left in leftward {
        if right.comb == comb::SIBLING {
            /* Reverse adjacent: the immediately preceding sibling must match. */
            emit_positional_sibling(b, &mut steps, Axis::PrecedingSibling)?;
            emit_compound_step(b, &mut steps, Axis::SelfAxis, left.first, left.last)?;
        } else {
            emit_compound_step(
                b,
                &mut steps,
                reverse_axis(right.comb),
                left.first,
                left.last,
            )?;
        }
        right = left;
    }

    finish_path(b, steps)
}
