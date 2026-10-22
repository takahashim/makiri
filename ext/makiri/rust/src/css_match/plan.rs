//! What a query decides once, from its compiled selector, for the document
//! it walks: each simple selector's name resolved there (lazily, as
//! Lexbor's `entry->id` is) and the tag a lone chain's subject must have,
//! which refuses other candidates before the machine starts. The matching
//! machine ([`Query`](super::query::Query)) reads it and decides nothing of
//! it.

use super::compile::{Compiled, Step};
use super::dom::{CssNode, Host};
use super::scratch::Scratch;
use super::simple::{Name, Names};
use super::MatchFailure;

pub(super) struct QueryPlan<'d, N: CssNode<'d>> {
    /// The simple selectors' names, resolved in the walked document.
    names: Names<N::Host>,
    /// The document a walking query walks, which its names are resolved in;
    /// `None` where names are compared as bytes.
    walked: Option<N::Doc>,
    /// The resolved type selector of a lone top-level chain's rightmost
    /// compound: a candidate with another tag is refused before the machine
    /// starts. (Of the walked document, as every candidate is - [`Name`].)
    tag_filter: Option<Option<<N::Host as Host>::TagId>>,
}

impl<'d, N: CssNode<'d>> QueryPlan<'d, N> {
    /// The plan for `compiled`. With `walk_in`, the query walks that
    /// document: names are resolved lazily, each the first time a candidate
    /// reaches it, and the rightmost type selector of a lone chain now, for
    /// the tag filter. Without it (`matches?`, one candidate) names are
    /// compared as bytes: a lookup costs more than the one comparison it
    /// would save - measured even resolved lazily, `ul > li.item` 85 -> 117
    /// ns. Only a host that keeps ids resolves at all.
    pub(super) fn new(
        compiled: &Compiled<'_>,
        walk_in: Option<N::Doc>,
        scratch: &mut Scratch<N::Host>,
    ) -> Result<Self, MatchFailure> {
        let walked = walk_in.filter(|_| <N::Host as Host>::RESOLVES_NAMES);
        let mut names = Names::reuse(&mut scratch.names);
        if walked.is_some() {
            for _ in compiled.simples.as_slice() {
                names.push(Name::Unresolved)?;
            }
        }
        let mut tag_filter = None;
        if let (Some(doc), [chain]) = (walked, compiled.top.as_slice()) {
            if chain.len != 0 {
                let last = compiled.compound(*chain, chain.len() - 1)?;
                for i in last.start..last.end {
                    let Some(sel) = compiled.simples.as_slice().get(i as usize) else {
                        continue;
                    };
                    if let Name::Tag(id) = Name::resolve(sel, doc) {
                        names.set(i as usize, Name::Tag(id))?;
                        tag_filter = Some(id);
                        break;
                    }
                }
            }
        }
        Ok(QueryPlan {
            names,
            walked,
            tag_filter,
        })
    }

    /// Hand the names back to `scratch` for the next query.
    pub(super) fn give_back(self, scratch: &mut Scratch<N::Host>) {
        self.names.give_back(&mut scratch.names);
    }

    /// Whether the tag filter refuses `node` - `type_matches`' own verdict,
    /// taken before anything else is set up.
    #[inline]
    pub(super) fn refuses(&self, node: N) -> bool {
        match self.tag_filter {
            Some(id) => id.is_none() || node.tag_id() != id,
            None => false,
        }
    }

    /// Simple selector `i`'s [`Name`], resolved in the walked document the
    /// first time it is asked for.
    #[inline]
    pub(super) fn name(&mut self, i: u32, sel: &Step<'_>) -> Name<N::Host> {
        match (self.names.get_mut(i as usize), self.walked) {
            (Some(slot @ Name::Unresolved), Some(doc)) => {
                *slot = Name::resolve(sel, doc);
                *slot
            }
            (Some(n), _) => *n,
            (None, _) => Name::None,
        }
    }
}
