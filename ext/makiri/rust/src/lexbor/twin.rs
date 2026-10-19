//! A Lexbor document beside its arena copy, node for node.
//!
//! The CSS matcher takes the arena's nodes, and the engine it replaced
//! (`lexbor::selectors`) takes Lexbor's; holding the one to the other needs
//! the same tree on both sides. The copy (`adapter::arena_copy`) is that tree,
//! and positions stand in for identities: both documents are walked in the
//! same order (template contents included), and a node on one side is the
//! node at the same place in the other's walk. Reference builds only.

#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;

use crate::html::dom as arena;
use crate::lexbor::adapter::html as lex;

/// The arena copy of a Lexbor document, with the correspondence.
pub struct Twin {
    copy: arena::Document,
    lex: Vec<lex::RawNode>,
    arena: Vec<arena::RawNode>,
    by_lex: HashMap<usize, usize>,
    by_arena: HashMap<arena::RawNode, usize>,
}

impl Twin {
    /// Copy `ld` into a new arena document.
    pub fn new(ld: lex::HtmlDoc<'_>) -> Result<Twin, arena::DomOom> {
        let copy = crate::lexbor::adapter::arena_copy::copy_document(ld)?;
        let lex: Vec<lex::RawNode> = ld
            .as_node()
            .subtree_with_contents()
            .map(lex::RawNode::from)
            .collect();
        let ids: Vec<arena::RawNode> = arena::HtmlDoc::of(&copy)
            .as_node()
            .subtree_with_contents()
            .map(arena::RawNode::from)
            .collect();
        assert_eq!(lex.len(), ids.len(), "the copy's walk differs in length");
        let by_lex = lex
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_ptr() as usize, i))
            .collect();
        let by_arena = ids.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        Ok(Twin {
            copy,
            lex,
            arena: ids,
            by_lex,
            by_arena,
        })
    }

    /// The arena copy.
    pub fn doc(&self) -> arena::HtmlDoc<'_> {
        arena::HtmlDoc::of(&self.copy)
    }

    /// The arena node at Lexbor node `n`'s place.
    pub fn to_arena(&self, n: lex::RawNode) -> arena::HtmlNode<'_> {
        let at = self.by_lex[&(n.as_ptr() as usize)];
        self.doc().node(self.arena[at])
    }

    /// The Lexbor node at arena node `n`'s place.
    pub fn to_lexbor(&self, n: arena::HtmlNode<'_>) -> lex::RawNode {
        self.lex[self.by_arena[&arena::RawNode::from(n)]]
    }
}
