//! What a query decides once, from its compiled selector, for the document
//! it walks: each simple selector's name resolved there (lazily, as
//! Lexbor's `entry->id` is) and the tag a lone chain's subject must have,
//! which refuses other candidates before the machine starts. The matching
//! machine ([`Query`](super::query::Query)) reads it and decides nothing of
//! it.

use super::compile::{Compiled, Step};
use super::dom::{CssDoc, CssNode, Host};
use super::scratch::{Scratch, Table};
use super::MatchFailure;
use crate::selectors::ast::Simple;

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
pub(super) enum Name<H: Host> {
    /// Not looked up yet.
    Unresolved,
    /// Nothing to look up: not a type or attribute selector.
    None,
    /// A type selector's tag id in the walked document; `None`: no element
    /// of the document has the name.
    Tag(Option<H::TagId>),
    /// An attribute selector's name.
    Attr(H::AttrName),
}

impl<H: Host> Clone for Name<H> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<H: Host> Copy for Name<H> {}

impl<H: Host> Name<H> {
    pub(super) fn resolve<'d, D: CssDoc<'d, Host = H>>(sel: &Step<'_>, doc: D) -> Name<H> {
        match sel.simple {
            Simple::Type => Name::Tag(doc.tag_id(sel.name)),
            /* An attribute in a namespace is found by scanning
             * (`attr_value_in`), not by a resolved name. */
            Simple::Attribute(_) if !sel.ns.is_uri() => Name::Attr(doc.resolve_attr_name(sel.name)),
            _ => Name::None,
        }
    }
}

/// A query's [`Name`]s, by simple-selector index: one per simple
/// selector, each resolved when it is first reached.
pub(super) type Names<H> = Table<Name<H>>;
