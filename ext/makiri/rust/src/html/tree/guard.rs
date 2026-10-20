//! The guard every HTML parse runs under, as the tree builder's own: what
//! `lexbor::adapter::tree_guard` does from a hook around Lexbor's tree
//! builder, done after each token of
//! [`TreeBuilder::process`](super::TreeBuilder), so the answers are the
//! same. The why of both limits is in `tree_guard`'s module doc. The
//! source-position stamps run at the same points and are `stamp.rs`'s.
//!
//! The checks stay per token, not per insertion: a token can push past the
//! limit and pop back (a void element at the limit), and the hook never saw
//! that, so a check at the insertion would refuse inputs the hook accepts.

use super::select::SelectWalk;
use super::{Tok, TreeBuilder};
use crate::html::dom::NodeId;
use crate::html::res::names as res;

/// The most `<option>`s one `<select>` may receive during a parse; real lists
/// are a few hundred at most.
pub const MAX_SELECT_OPTIONS: usize = 10_000;

/// What stopped a parse the guard refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardStop {
    /// The tree grew deeper than the [`DepthLimit`] allowed.
    TooDeep,
    /// One `<select>` received more than [`MAX_SELECT_OPTIONS`] options.
    TooManyOptions,
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
    /// parser keeps below the first real element.
    pub(crate) fn max_open(self, synthetic: usize) -> usize {
        self.0.map_or(usize::MAX, |d| d.saturating_add(synthetic))
    }
}

/// Whether a fragment's context leads an option at the fragment's top level
/// to a `<select>` - from where the walk arrives with no `<optgroup>` seen
/// (`fresh`) and with one (`after_optgroup`). `tree_guard::OptionContext`'s
/// `Resolved`; a document parse has no context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextOptions {
    pub fresh: bool,
    pub after_optgroup: bool,
}

/// The guard's state across a parse.
pub(crate) struct Guard {
    /// The stack length above which the parse stops; `usize::MAX` for none.
    max_open: usize,
    /// The `<select>` the last counted option went into (`None` for the
    /// context's), whether there was one, and how many options it has had.
    select: Option<Option<NodeId>>,
    options: usize,
    context: Option<ContextOptions>,
    pub(crate) stopped: Option<GuardStop>,
}

impl Default for Guard {
    fn default() -> Guard {
        Guard {
            max_open: usize::MAX,
            select: None,
            options: 0,
            context: None,
            stopped: None,
        }
    }
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
    pub fn for_fragment(limit: DepthLimit, context: ContextOptions) -> Guard {
        Guard {
            max_open: limit.max_open(1),
            context: Some(context),
            ..Guard::default()
        }
    }
}

impl TreeBuilder<'_> {
    /// After a token: refuse the parse when the tree is too deep or a
    /// select has too many options. `false` when the parse is to stop.
    pub(crate) fn guard_after(&mut self, tok: &Tok) -> bool {
        if self.guard.max_open != usize::MAX && self.open.len() > self.guard.max_open {
            self.guard.stopped = Some(GuardStop::TooDeep);
            return false;
        }
        let option_start = tok.tag_id == res::TAG_OPTION && !tok.is_close();
        if option_start && self.count_option() {
            self.guard.stopped = Some(GuardStop::TooManyOptions);
            return false;
        }
        true
    }

    /// `HookState::count_option`: count the `<option>` just inserted against
    /// the select it updates; whether that select is now past the limit.
    fn count_option(&mut self) -> bool {
        let Some(option) = self.current_node() else {
            return false;
        };
        if !self.node_is(option, res::TAG_OPTION) {
            return false;
        }
        let Some(key) = self.guard_nearest_select(option) else {
            return false;
        };
        if self.guard.select == Some(key) {
            self.guard.options += 1;
        } else {
            self.guard.select = Some(key);
            self.guard.options = 1;
        }
        self.guard.options > MAX_SELECT_OPTIONS
    }

    /// `tree_guard::nearest_select`: the select an inserted option updates,
    /// `Some(None)` for the fragment context's.
    fn guard_nearest_select(&self, option: NodeId) -> Option<Option<NodeId>> {
        match self.walk_to_select(option) {
            SelectWalk::Select(n) => Some(Some(n)),
            SelectWalk::Blocked => None,
            /* Off the top: in a fragment, the synthetic root, whose children
             * land in the context. */
            SelectWalk::OffTop { optgroup } => {
                let c = self.guard.context?;
                let reaches = if optgroup { c.after_optgroup } else { c.fresh };
                reaches.then_some(None)
            }
        }
    }
}
