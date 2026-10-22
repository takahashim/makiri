//! Compiling a selector: every chain - the top-level comma alternatives and
//! every list nested in `:is()`/`:where()`/`:not()`/`:has()`/`of S` - split
//! into compounds once, into query-local tables, before any node is tested;
//! and the one place a selector this matcher will not run is refused.

use crate::selectors::ast::{
    CaseModifier, Combinator, FunctionArg, List, ListPseudo, Lists, Simple,
};

use core::ffi::c_long;

use super::ns::{Namespaces, NsTest};
use super::scratch::{Table, Tables};
use super::{MatchFailure, MAX_COMPOUNDS};

/// A compound (a `Close`-linked run of simple selectors, `simples[start ..
/// end]` of the [`Compiled`] table, in written order) plus the combinator
/// that attaches it to the compound BEFORE it (to its left - more
/// ancestor/earlier-sibling-ward) in the written selector. The same index
/// keeps a [`Query`](super::query::Query)'s resolved names.
#[derive(Clone, Copy)]
pub(super) struct Compound {
    pub(super) start: u32,
    pub(super) end: u32,
    pub(super) comb: Combinator,
}

/// A compound chain: `compounds[start .. start + len]` of the query's
/// [`Compiled`] table, left to right as written. `len == 0` is an empty
/// chain, which never matches.
///
/// `Copy`: two integers into the table, which a task carries as it is.
#[derive(Clone, Copy, Default)]
pub(super) struct Chain {
    pub(super) start: u32,
    pub(super) len: u32,
}

impl Chain {
    pub(super) fn len(self) -> usize {
        self.len as usize
    }
}

/// One simple selector, decoded once at compile time - Lexbor's own
/// `entry->selector`, read through its `type` switch per candidate, is what
/// this saves re-decoding on every node.
#[derive(Clone, Copy)]
pub(super) struct Step<'p> {
    pub(super) simple: Simple<'p>,
    pub(super) name: &'p [u8],
    /// A list-pseudo's or `of S`'s alternatives: `alts[alts ..
    /// alts + n_alts]` of the [`Compiled`] table.
    pub(super) alts: u32,
    pub(super) n_alts: u32,
    /// An attribute selector's value comparison: `Some(ci)` settled at compile
    /// time (an `i` / `s` modifier, or a name outside the table in
    /// [`is_html_ci_attribute`]), `None` for a table name, case-insensitive on
    /// an HTML element only - Lexbor's per-id `switch`, decided once rather
    /// than per node.
    pub(super) value_ci: Option<bool>,
    /// An `:is()` / `:where()` / `:not()` whose every alternative is one
    /// compound with nothing nested in it: answered in place by
    /// [`Query::check_compound`](super::query::Query::check_compound), with no task.
    pub(super) inline: bool,
    /// The namespace a type, universal or attribute selector asks for,
    /// resolved against the query's [`Namespaces`]; [`NsTest::Any`] for
    /// every other kind.
    pub(super) ns: NsTest,
    /// Whether - and how - this simple selector is answered by a nested
    /// selector list: decided once, here, and read by everything that
    /// treats a nested selector differently (`check_simple`'s deferral,
    /// [`Compiled::compound_is_flat`], `mark_inline`, `Query::deferred_task`).
    pub(super) nest: Nest,
}

impl Step<'_> {
    /// The `*` a compound with no type selector takes under a default
    /// namespace (`ns`, `NsTest::implied`), as if it were written first.
    fn implied_universal(ns: NsTest) -> Self {
        Step {
            simple: Simple::Universal,
            name: b"",
            ns,
            alts: 0,
            n_alts: 0,
            value_ci: Some(false),
            inline: false,
            nest: Nest::None,
        }
    }
}

/// How a simple selector with a nested selector list is answered - the one
/// classification of them ([`nest_of`]). A new list-bearing pseudo-class is
/// added here and in `Query::deferred_task`, which builds its task.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Nest {
    /// No nested list: the simple selector answers itself.
    None,
    /// `:is()` / `:where()` (`negate` false) or `:not()` (`negate` true):
    /// one of the alternatives matches the node.
    List { negate: bool },
    /// `:has()`: a forward search from the node.
    Has,
    /// `:nth-child(An+B of S)` / `:nth-last-child(...)`: the node's rank among
    /// the siblings `S` matches.
    NthOf {
        a: c_long,
        b: c_long,
        from_end: bool,
    },
}

/// `simple`'s nested selector lists and how they answer it - or `Unsupported`
/// for `:current()`, which this matcher never evaluates, and for a
/// `:lexbor-contains()` (which the parser rejects as an unknown function).
fn nest_of<'p>(simple: Simple<'p>) -> Result<(Nest, Option<Lists<'p>>), MatchFailure> {
    Ok(match simple {
        Simple::PseudoClassFunction(FunctionArg::Contains | FunctionArg::Current(_)) => {
            return Err(MatchFailure::Unsupported)
        }
        Simple::PseudoClassFunction(FunctionArg::Selectors { pseudo, lists }) => {
            let nest = match pseudo {
                ListPseudo::Has => Nest::Has,
                ListPseudo::Not => Nest::List { negate: true },
                _ => Nest::List { negate: false },
            };
            (nest, Some(lists))
        }
        Simple::PseudoClassFunction(FunctionArg::Nth {
            from_end,
            anb: Some(anb),
            ..
        }) if anb.of.is_some() => (
            Nest::NthOf {
                a: anb.a,
                b: anb.b,
                from_end,
            },
            anb.of,
        ),
        _ => (Nest::None, None),
    })
}

/// Where a chain sits, for the two rules that depend on it.
#[derive(Clone, Copy)]
struct ChainPlace {
    /// The chain is an alternative of `:is()` / `:where()` / `:not()`, whose
    /// subject compound takes no default namespace (Selectors 4 §4.2-4.4)
    /// unless it writes a type or universal selector.
    list_arg: bool,
    /// The alternative slot (`alts`) of the nearest forgiving alternative
    /// around the chain (`:is()` / `:where()`, Selectors 4 §4.2), which an
    /// invalid selector inside it empties; `None` where one fails the query.
    within: Option<u32>,
}

impl ChainPlace {
    /// A top-level comma alternative.
    const TOP: ChainPlace = ChainPlace {
        list_arg: false,
        within: None,
    };
}

/// A nested list on [`compile`]'s work list: what its alternatives' places
/// are made from ([`Nesting::place_of`]).
#[derive(Clone, Copy)]
pub(super) struct Nesting {
    list_arg: bool,
    /// The list is forgiving: each alternative is its own `within`.
    forgiving: bool,
    /// The place of the chain the list is written in.
    within: Option<u32>,
}

impl Nesting {
    /// The list of `simple` (whose nesting is `nest`), written in a chain
    /// at `place`.
    fn of(simple: Simple<'_>, nest: Nest, place: ChainPlace) -> Nesting {
        Nesting {
            list_arg: matches!(nest, Nest::List { .. }),
            forgiving: matches!(
                simple,
                Simple::PseudoClassFunction(FunctionArg::Selectors {
                    pseudo: ListPseudo::Is | ListPseudo::Where,
                    ..
                })
            ),
            within: place.within,
        }
    }

    /// The place of the alternative in slot `k`.
    fn place_of(self, k: u32) -> ChainPlace {
        ChainPlace {
            list_arg: self.list_arg,
            within: if self.forgiving { Some(k) } else { self.within },
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
pub(super) struct Compiled<'p> {
    /// Every simple selector, compound by compound, in the order the
    /// compounds number them.
    pub(super) simples: Table<Step<'p>>,
    pub(super) compounds: Table<Compound>,
    /// The top-level comma alternatives, in order.
    pub(super) top: Table<Chain>,
    /// Every nested list's alternatives, each list's in order ([`Step::alts`]).
    pub(super) alts: Table<Chain>,
    /// The bindings the steps' [`NsTest`]s point into.
    pub(super) ns: Namespaces<'p>,
}

/// Compile `groups` - see [`Compiled`] - or refuse it: no chain anywhere
/// over [`MAX_COMPOUNDS`], no `:current()` or column
/// combinator (`||`) anywhere, checked over EVERY comma alternative and EVERY nested
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
/// - **An unsupported construct used to be found only if matching reached
///   it.** A type mismatch earlier in the SAME compound, an earlier comma
///   alternative that already answered, or `at_css`'s first-match stop could
///   each skip the construct: `nosuch:lexbor-contains("x")` (which no longer
///   parses at all) answered empty while `p:lexbor-contains("x")` raised, and
///   `p, nosuch:lexbor-contains("x")` answered the `<p>`s. The exception must not depend on document content,
///   comma-alternative order, or `at_css`'s early termination.
///
/// Iterative: nested lists wait on an explicit work list, so a selector
/// nested 500,000 deep costs heap, not native stack. A recursive first
/// version turned `:is()` nested 2000 deep into a `SystemStackError` in a
/// 128 KiB `Fiber`, which wedged the shared CSS engine for the process.
///
/// The tables are `scratch`'s, lent for the query ([`Compiled::give_back`]
/// returns them).
pub(super) fn compile<'p>(
    scratch: &mut Tables,
    groups: Lists<'p>,
    ns: &Namespaces<'p>,
) -> Result<Compiled<'p>, MatchFailure> {
    let mut c = Compiled {
        simples: Table::reuse(&mut scratch.simples),
        compounds: Table::reuse(&mut scratch.compounds),
        top: Table::reuse(&mut scratch.top),
        alts: Table::reuse(&mut scratch.alts),
        ns: *ns,
    };
    let mut pending: Table<(Lists<'p>, u32, Nesting)> = Table::reuse(&mut scratch.pending);
    let built = (|| {
        for list in groups {
            let chain = c.add_chain(list, ns, ChainPlace::TOP, &mut pending)?;
            c.top.push(chain)?;
        }
        while let Some((lists, at, nesting)) = pending.pop() {
            for (k, list) in (at..).zip(lists) {
                let place = nesting.place_of(k);
                match c.add_chain(list, ns, place, &mut pending) {
                    Ok(chain) => c.alts.set(k as usize, chain)?,
                    /* An undeclared prefix makes a selector invalid, and a
                     * forgiving list drops an invalid alternative alone
                     * (Selectors 4 §4.2): the empty chain matches nothing.
                     * Its own nested lists are still compiled, and can only
                     * empty it again. Anything else - out of memory, too
                     * complex - fails the query. */
                    Err(MatchFailure::UndeclaredPrefix) => match place.within {
                        Some(slot) => c.alts.set(slot as usize, Chain::default())?,
                        None => return Err(MatchFailure::UndeclaredPrefix),
                    },
                    Err(e) => return Err(e),
                }
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

/// [`compile`]'s verdict alone, for tests that check it without matching.
#[cfg(all(test, feature = "lexbor"))]
pub(crate) fn validate(groups: Lists<'_>) -> Result<(), MatchFailure> {
    compile(&mut Tables::new(), groups, &Namespaces::NONE).map(drop)
}

impl<'p> Compiled<'p> {
    /// Split `list`'s chain into compounds, left to right (the order
    /// the parser links them in), appending them to `compounds`; any list
    /// nested in its simple selectors is queued on `pending`, not walked
    /// here. `place` is where the chain sits.
    fn add_chain(
        &mut self,
        list: List<'p>,
        ns: &Namespaces<'p>,
        place: ChainPlace,
        pending: &mut Table<(Lists<'p>, u32, Nesting)>,
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
            if matches!(comb, Combinator::Column | Combinator::Other) {
                return Err(MatchFailure::Unsupported);
            }
            let simple_start = self.simple_index()?;
            if let Some(implied) =
                NsTest::implied(ns).filter(|_| takes_implied_namespace(first, place))
            {
                self.simples.push(Step::implied_universal(implied))?;
            }
            let mut sel = first;
            loop {
                let simple = sel.simple();
                let (nest, nested) = nest_of(simple)?;
                let alts = u32::try_from(self.alts.len()).map_err(|_| MatchFailure::TooComplex)?;
                let mut n_alts = 0u32;
                if let Some(lists) = nested {
                    // Slots now, filled when `pending` reaches the list.
                    for _ in lists {
                        self.alts.push(Chain::default())?;
                        n_alts += 1;
                    }
                    pending.push((lists, alts, Nesting::of(simple, nest, place)))?;
                }
                let value_ci = match simple {
                    Simple::Attribute(at) if at.case == CaseModifier::Insensitive => Some(true),
                    Simple::Attribute(at) if at.case == CaseModifier::Sensitive => Some(false),
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
                    ns: NsTest::resolve(sel, simple, ns)?,
                    alts,
                    n_alts,
                    value_ci,
                    inline: false,
                    nest,
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
    pub(super) fn give_back(self, scratch: &mut Tables) {
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
            /* `:is()` / `:where()` / `:not()` only: a `:has()` searches
             * forward from the node, which no compound check can do. */
            let Nest::List { .. } = sel.nest else {
                continue;
            };
            let mut inline = true;
            for k in sel.alts..sel.alts + sel.n_alts {
                let chain = self.alts.get(k as usize).ok_or(MatchFailure::Internal)?;
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

    /// Nothing in `compound` has a nested list, so nothing in it defers
    /// ([`check_simple`](super::simple::check_simple)'s `Deferred` is exactly
    /// a step whose [`Step::nest`] is not [`Nest::None`]).
    fn compound_is_flat(&self, compound: Compound) -> Result<bool, MatchFailure> {
        let steps = self
            .simples
            .as_slice()
            .get(compound.start as usize..compound.end as usize)
            .ok_or(MatchFailure::Internal)?;
        Ok(steps.iter().all(|s| s.nest == Nest::None))
    }

    fn simple_index(&self) -> Result<u32, MatchFailure> {
        u32::try_from(self.simples.len()).map_err(|_| MatchFailure::TooComplex)
    }

    /// `chain[idx]`. Out of range is a broken invariant, raised rather than
    /// answered.
    pub(super) fn compound(&self, chain: Chain, idx: usize) -> Result<Compound, MatchFailure> {
        self.compounds
            .get(chain.start as usize + idx)
            .ok_or(MatchFailure::Internal)
    }
}

/// Whether the compound `first` begins, in a chain at `place`, is kept to
/// the default namespace (Selectors 4 §5.3): it writes no type or universal
/// selector - one only begins a compound (C28) - and is not the subject of
/// an `:is()` / `:where()` / `:not()` alternative.
fn takes_implied_namespace(first: crate::selectors::ast::Selector<'_>, place: ChainPlace) -> bool {
    let typed = matches!(first.simple(), Simple::Type | Simple::Universal);
    !typed && !(place.list_arg && is_subject(first))
}

/// Whether the compound `first` begins is its chain's last - the subject -
/// by a walk over the compound's own links.
fn is_subject(first: crate::selectors::ast::Selector<'_>) -> bool {
    let mut sel = first;
    loop {
        match sel.next() {
            Some(n) if n.combinator() == Combinator::Close => sel = n,
            next => return next.is_none(),
        }
    }
}

/// The 46 HTML attributes whose VALUE compares ASCII case-insensitively
/// (`lxb_selectors_match_attribute_html_case_insensitive`) when no `i`/`s`
/// modifier is written, and only on an HTML-namespace element of an HTML
/// document (`attribute_matches`). Asked once per selector, by `compile`.
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
