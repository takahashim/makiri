//! Compiling a selector: every chain - the top-level comma alternatives and
//! every list nested in `:is()`/`:where()`/`:not()`/`:has()`/`of S` - split
//! into compounds once, into query-local tables, before any node is tested;
//! and the one place a selector this matcher will not run is refused.

use crate::lexbor::css_parser::{Combinator, FunctionArg, List, ListPseudo, Lists, Simple};

use super::scratch::{Scratch, Table};
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
///
/// The tables are `scratch`'s, lent for the query ([`Compiled::give_back`]
/// returns them).
pub(super) fn compile<'p>(
    scratch: &mut Scratch,
    groups: Lists<'p>,
) -> Result<Compiled<'p>, MatchFailure> {
    let mut c = Compiled {
        simples: Table::reuse(&mut scratch.simples),
        compounds: Table::reuse(&mut scratch.compounds),
        top: Table::reuse(&mut scratch.top),
        alts: Table::reuse(&mut scratch.alts),
    };
    let mut pending: Table<(Lists<'p>, u32)> = Table::reuse(&mut scratch.pending);
    let built = (|| {
        for list in groups {
            let chain = c.add_chain(list, &mut pending)?;
            c.top.push(chain)?;
        }
        while let Some((lists, at)) = pending.pop() {
            for (k, list) in (at..).zip(lists) {
                let chain = c.add_chain(list, &mut pending)?;
                c.alts.set(k as usize, chain)?;
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

/// [`compile`]'s verdict alone, over `scratch`'s tables - for `matches?` on
/// a node no selector can match, which still refuses a selector the matcher
/// would refuse.
pub fn check_compiles(scratch: &mut Scratch, groups: Lists<'_>) -> Result<(), MatchFailure> {
    compile(scratch, groups).map(|c| c.give_back(scratch))
}

/// [`compile`]'s verdict alone, for tests that check it without matching.
#[cfg(test)]
pub(crate) fn validate(groups: Lists<'_>) -> Result<(), MatchFailure> {
    compile(&mut Scratch::new(), groups).map(drop)
}

impl<'p> Compiled<'p> {
    /// Split `list`'s chain into compounds, left to right (the order
    /// `css_parser` links them in), appending them to `compounds`; any list
    /// nested in its simple selectors is queued on `pending`, not walked
    /// here.
    fn add_chain(
        &mut self,
        list: List<'p>,
        pending: &mut Table<(Lists<'p>, u32)>,
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
            let simple_start = self.simple_index()?;
            let mut sel = first;
            loop {
                let simple = sel.simple();
                let nested = match simple {
                    Simple::PseudoClassFunction(FunctionArg::Contains(_)) => {
                        return Err(MatchFailure::Unsupported)
                    }
                    Simple::PseudoClassFunction(FunctionArg::Selectors { lists, .. }) => {
                        Some(lists)
                    }
                    Simple::PseudoClassFunction(FunctionArg::Nth { anb, .. }) => {
                        anb.and_then(|a| a.of_list)
                    }
                    _ => None,
                };
                let alts = u32::try_from(self.alts.len()).map_err(|_| MatchFailure::TooComplex)?;
                let mut n_alts = 0u32;
                if let Some(lists) = nested {
                    // Slots now, filled when `pending` reaches the list.
                    for _ in lists {
                        self.alts.push(Chain::default())?;
                        n_alts += 1;
                    }
                    pending.push((lists, alts))?;
                }
                let value_ci = match simple {
                    Simple::Attribute(at) if at.case_insensitive => Some(true),
                    Simple::Attribute(at) if at.explicit_sensitive => Some(false),
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
                    alts,
                    n_alts,
                    value_ci,
                    inline: false,
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
    pub(super) fn give_back(self, scratch: &mut Scratch) {
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
            let Simple::PseudoClassFunction(FunctionArg::Selectors { pseudo, .. }) = sel.simple
            else {
                continue;
            };
            if pseudo == ListPseudo::Has {
                continue;
            }
            let mut inline = true;
            for k in sel.alts..sel.alts + sel.n_alts {
                let chain = self.alts.get(k as usize).ok_or(MatchFailure::Unsupported)?;
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

    /// Nothing in `compound` defers ([`check_simple`](super::simple::check_simple)'s `Deferred`).
    fn compound_is_flat(&self, compound: Compound) -> Result<bool, MatchFailure> {
        let steps = self
            .simples
            .as_slice()
            .get(compound.start as usize..compound.end as usize)
            .ok_or(MatchFailure::Unsupported)?;
        Ok(steps.iter().all(|s| match s.simple {
            Simple::PseudoClassFunction(FunctionArg::Selectors { .. }) => false,
            Simple::PseudoClassFunction(FunctionArg::Nth { anb, .. }) => {
                anb.and_then(|a| a.of_list).is_none()
            }
            _ => true,
        }))
    }

    fn simple_index(&self) -> Result<u32, MatchFailure> {
        u32::try_from(self.simples.len()).map_err(|_| MatchFailure::TooComplex)
    }

    /// `chain[idx]`. Out of range is a broken invariant, raised rather than
    /// answered.
    pub(super) fn compound(&self, chain: Chain, idx: usize) -> Result<Compound, MatchFailure> {
        self.compounds
            .get(chain.start as usize + idx)
            .ok_or(MatchFailure::Unsupported)
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
