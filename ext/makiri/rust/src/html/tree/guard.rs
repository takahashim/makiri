//! The guard every HTML parse runs under, as the tree builder's own: what
//! `lexbor::adapter::tree_guard` does from a hook around Lexbor's tree
//! builder, done after each token of
//! [`TreeBuilder::process`](super::TreeBuilder), so the answers are the
//! same. The why of the depth limit is in `tree_guard`'s module doc. The
//! source-position stamps run at the same points and are `stamp.rs`'s.
//!
//! There is no limit on a `<select>`'s options any more: the 10,000 that
//! stood here held off the selectedness setting algorithm's walk of every
//! option per option inserted, which `select.rs` now does without.
//!
//! The checks stay per token, not per insertion: a token can push past the
//! limit and pop back (a void element at the limit), and the hook never saw
//! that, so a check at the insertion would refuse inputs the hook accepts.

use super::TreeBuilder;

/// What stopped a parse the guard refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardStop {
    /// The tree grew deeper than the [`DepthLimit`] allowed.
    TooDeep,
}

/// The deepest element an HTML parse accepts, or no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthLimit(Option<usize>);

impl DepthLimit {
    /// Nokogiri::HTML5's default (`Nokogiri::Gumbo::DEFAULT_MAX_TREE_DEPTH`).
    pub const DEFAULT: DepthLimit = DepthLimit(Some(400));
    /// No limit: the quadratic worst case is back.
    pub const UNLIMITED: DepthLimit = DepthLimit(None);

    /// Accept elements up to depth `depth`: an element's depth counts itself
    /// and its ancestors, `<html>` being 1 in a document and the top level 1
    /// in a fragment.
    pub const fn at_most(depth: usize) -> DepthLimit {
        DepthLimit(Some(depth))
    }

    /// The limit, or `None` when there is none.
    pub fn max_depth(self) -> Option<usize> {
        self.0
    }

    /// The longest open-element stack allowed, given `synthetic` entries the
    /// parser keeps below the first real element; `None` for no limit.
    pub(crate) fn max_open(self, synthetic: usize) -> Option<usize> {
        self.0.map(|d| d.saturating_add(synthetic))
    }
}

/// The guard's state across a parse.
#[derive(Default)]
pub(crate) struct Guard {
    /// The stack length above which the parse stops; `None` for no limit.
    max_open: Option<usize>,
    pub(crate) stopped: Option<GuardStop>,
}

impl Guard {
    /// A document parse's guard: `limit` from the root.
    pub fn for_document(limit: DepthLimit) -> Guard {
        Guard {
            max_open: limit.max_open(0),
            ..Guard::default()
        }
    }

    /// A fragment parse's guard: `limit` over the fragment's own elements,
    /// below the parser's synthetic `<html>`.
    pub fn for_fragment(limit: DepthLimit) -> Guard {
        Guard {
            max_open: limit.max_open(1),
            ..Guard::default()
        }
    }
}

impl TreeBuilder<'_> {
    /// After a token: refuse the parse when the tree is too deep. `false`
    /// when the parse is to stop.
    pub(crate) fn guard_after(&mut self) -> bool {
        if self.guard.max_open.is_some_and(|max| self.open.len() > max) {
            self.guard.stopped = Some(GuardStop::TooDeep);
            return false;
        }
        true
    }
}
