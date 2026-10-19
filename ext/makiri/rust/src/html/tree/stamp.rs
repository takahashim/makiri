//! The source-position stamps, as the tree builder's own: what
//! `lexbor::adapter::source_loc` does from a hook around Lexbor's tree
//! builder, done around [`TreeBuilder::process`](super::TreeBuilder) at the
//! same points, before and after each token. The limits the parse is held
//! to are the guard's (`guard.rs`), which runs at the same points and knows
//! nothing of these.

use super::{Tok, TreeBuilder};
use crate::compat::{self, Fix};
use crate::html::dom::NodeId;
use crate::html::res::names as res;

/// Whether a parse stamps elements with their start tag's offset, and the
/// input's length, which an offset must be below. A document parse does;
/// fragments and the unstamped [`parse_document`](super::parse_document)
/// do not.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Stamping {
    pub(crate) input_len: Option<usize>,
}

impl Stamping {
    /// Stamp a document of `input_len` bytes.
    pub(crate) fn document(input_len: usize) -> Stamping {
        Stamping {
            input_len: Some(input_len),
        }
    }
}

/// An element start tag the tree builder is about to process, and the tree
/// just before: the current node and the last child of where it inserts
/// (`source_loc::StartTag` and `Before`).
pub(crate) struct Before {
    tag: usize,
    offset: usize,
    current: Option<NodeId>,
    last_child: Option<NodeId>,
}

impl TreeBuilder<'_> {
    /// Where the tree builder appends a child of `n`: an HTML `<template>`'s
    /// contents, otherwise `n`.
    fn inserts_into(&self, n: NodeId) -> NodeId {
        self.doc.template_content(n).unwrap_or(n)
    }

    /// The offset an element created from `tok` carries: its start tag's,
    /// when `tok` is a real start tag of a stamping parse - not a token the
    /// tree builder made up (`Tok::fake`) or one that copies an existing
    /// element (`base`: the adoption agency, the reconstruction of the
    /// active formatting elements). The HTML Standard's sense of an element
    /// "created for the token"; L1 in docs/parser_differences.ja.md.
    pub(crate) fn created_offset(&self, tok: &Tok) -> Option<usize> {
        let len = self.stamping.input_len?;
        if !compat::fixed(Fix::L1) || tok.base.is_some() {
            return None;
        }
        tok.begin.filter(|&o| o < len)
    }

    /// Before a token: the stamp's snapshot, for an element start tag
    /// (`Stamper::start_tag`, `Before::at`) - Lexbor's rule, which only the
    /// Lexbor-compatible run (`compat::as_lexbor`) still uses.
    pub(crate) fn stamp_before(&self, tok: &Tok) -> Option<Before> {
        let len = self.stamping.input_len?;
        if compat::fixed(Fix::L1) || tok.tag_id <= res::TAG__EM_DOCTYPE || tok.is_close() {
            return None;
        }
        let offset = tok.begin?;
        if offset >= len {
            return None;
        }
        let current = self.current_node();
        Some(Before {
            tag: tok.tag_id,
            offset,
            current,
            last_child: current.and_then(|c| self.doc.last_child(self.inserts_into(c))),
        })
    }

    /// After a token: stamp the element it created for the start tag
    /// [`stamp_before`](Self::stamp_before) saw (`source_loc::stamp_created`).
    pub(crate) fn stamp_after(&mut self, before: Option<Before>) {
        let Some(b) = before else { return };
        let Some(now) = self.current_node() else {
            return;
        };
        let candidate = if Some(now) != b.current && self.is_element_tag(now, b.tag) {
            Some(now)
        } else {
            self.doc.last_child(self.inserts_into(now))
        };
        let Some(x) = candidate else {
            return;
        };
        if Some(x) == b.current
            || Some(x) == b.last_child
            || !self.is_element_tag(x, b.tag)
            || self.doc.source_offset(x).is_some()
            || self.doc.first_child(x).is_some()
        {
            return;
        }
        self.doc.set_source_offset(x, b.offset);
    }

    /// Whether `n` is an element with tag id `tag`.
    fn is_element_tag(&self, n: NodeId, tag: usize) -> bool {
        self.doc.element(n).is_some() && self.tag(n) == tag
    }
}
