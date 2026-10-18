//! The guard every HTML parse runs under, and the source-position stamps,
//! as the tree builder's own: what `lexbor::adapter::tree_guard` and
//! `lexbor::adapter::source_loc` do from a hook around Lexbor's tree builder,
//! done around [`TreeBuilder::process`](super::TreeBuilder) at the same
//! points - before and after each token - so the answers are the same. The
//! why of both limits is in `tree_guard`'s module doc.
//!
//! The checks stay per token, not per insertion: a token can push past the
//! limit and pop back (a void element at the limit), and the hook never saw
//! that, so a check at the insertion would refuse inputs the hook accepts.

use super::compat::{self, Fix};
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
    /// Whether elements get their start tag's offset (a document parse).
    stamp: bool,
    /// The input's length, which an offset must be below.
    input_len: usize,
}

impl Default for Guard {
    fn default() -> Guard {
        Guard {
            max_open: usize::MAX,
            select: None,
            options: 0,
            context: None,
            stopped: None,
            stamp: false,
            input_len: 0,
        }
    }
}

impl Guard {
    /// A document parse's guard: `limit` from the root, stamping offsets.
    pub fn for_document(limit: DepthLimit, input_len: usize) -> Guard {
        Guard {
            max_open: limit.max_open(0),
            stamp: true,
            input_len,
            ..Guard::default()
        }
    }

    /// A fragment parse's guard: `limit` over the fragment's own elements,
    /// below the parser's synthetic `<html>`; no stamps.
    pub fn for_fragment(limit: DepthLimit, context: ContextOptions) -> Guard {
        Guard {
            max_open: limit.max_open(1),
            context: Some(context),
            ..Guard::default()
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
        if !self.guard.stamp || !compat::fixed(Fix::L1) || tok.base.is_some() {
            return None;
        }
        tok.begin.filter(|&o| o < self.guard.input_len)
    }

    /// Before a token: the stamp's snapshot, for an element start tag
    /// (`Stamper::start_tag`, `Before::at`) - Lexbor's rule, which only the
    /// Lexbor-compatible run (`compat::as_lexbor`) still uses.
    pub(crate) fn guard_before(&self, tok: &Tok) -> Option<Before> {
        if !self.guard.stamp
            || compat::fixed(Fix::L1)
            || tok.tag_id <= res::TAG__EM_DOCTYPE
            || tok.is_close()
        {
            return None;
        }
        let offset = tok.begin?;
        if offset >= self.guard.input_len {
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

    /// After a token: stamp what it created, then refuse the parse when the
    /// tree is too deep or a select has too many options. `false` when the
    /// parse is to stop.
    pub(crate) fn guard_after(&mut self, before: Option<Before>, tok: &Tok) -> bool {
        if let Some(b) = before {
            self.stamp_created(&b);
        }
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

    /// `source_loc::stamp_created`.
    fn stamp_created(&mut self, b: &Before) {
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
        let mut optgroup = false;
        let mut node = self.doc.parent(option);
        while let Some(n) = node {
            if self.ns(n) == res::NS_HTML {
                match self.tag(n) {
                    res::TAG_DATALIST | res::TAG_HR | res::TAG_OPTION => return None,
                    res::TAG_OPTGROUP if optgroup => return None,
                    res::TAG_OPTGROUP => optgroup = true,
                    res::TAG_SELECT => return Some(Some(n)),
                    _ => {}
                }
            }
            node = self.doc.parent(n);
        }
        /* Off the top: in a fragment, the synthetic root, whose children land
         * in the context. */
        let c = self.guard.context?;
        let reaches = if optgroup { c.after_optgroup } else { c.fresh };
        reaches.then_some(None)
    }
}
