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
//!
//! The second check bounds what a parse BUILDS against what it read: the
//! document may grow to [`ParseLimits::max_bytes`] - its input times the
//! amplification limit, or [`AMPLIFICATION_FLOOR`], whichever is more. The
//! HTML Standard's "reconstruct the active formatting elements" re-creates
//! every formatting element the stack lost, with its attributes, before each
//! character: ~390 `<b>`s with distinct attributes (Noah's Ark keeps only
//! identical ones to three) followed by `<div>x</div>` repeated built 30
//! nodes a byte - a 62 KB input became a 344 MB document. Ordinary documents
//! build 10-46 times their input, the default limit is 256, and the check is
//! O(1) per token ([`Document::approx_bytes`](crate::html::dom::Document)).
//! The tree of a parse it lets through is the Standard's, unchanged; one it
//! stops fails closed.

use super::TreeBuilder;

/// What stopped a parse the guard refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardStop {
    /// The tree grew deeper than the [`ParseLimits`] allowed.
    TooDeep,
    /// The document grew past its amplification limit
    /// ([`ParseLimits::max_bytes`]).
    TooLarge,
}

/// The least a parse may build, however short its input: the amplification
/// limit is a ratio, and a small input's legitimate document is not refused
/// for it.
pub const AMPLIFICATION_FLOOR: usize = 64 << 20;

/// What an HTML parse accepts: how deep the tree may grow, and how large
/// against its input. `None` is no limit for either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseLimits {
    depth: Option<usize>,
    amplification: Option<usize>,
}

impl ParseLimits {
    /// The depth is Nokogiri::HTML5's default
    /// (`Nokogiri::Gumbo::DEFAULT_MAX_TREE_DEPTH`); the amplification limit
    /// is Makiri's, see the module doc.
    pub const DEFAULT: ParseLimits = ParseLimits {
        depth: Some(400),
        amplification: Some(256),
    };
    /// No limit: the quadratic worst case is back, and so is the unbounded
    /// document a short input can build.
    pub const UNLIMITED: ParseLimits = ParseLimits {
        depth: None,
        amplification: None,
    };

    /// The default, accepting elements up to depth `depth`: an element's
    /// depth counts itself and its ancestors, `<html>` being 1 in a document
    /// and the top level 1 in a fragment.
    pub const fn at_most(depth: usize) -> ParseLimits {
        ParseLimits::DEFAULT.with_depth(Some(depth))
    }

    /// These limits with the depth `depth` (`None`: none).
    pub const fn with_depth(self, depth: Option<usize>) -> ParseLimits {
        ParseLimits { depth, ..self }
    }

    /// These limits with the amplification limit `ratio` (`None`: none).
    pub const fn with_amplification(self, ratio: Option<usize>) -> ParseLimits {
        ParseLimits {
            amplification: ratio,
            ..self
        }
    }

    /// The depth limit, or `None` when there is none.
    pub fn max_depth(self) -> Option<usize> {
        self.depth
    }

    /// The amplification limit, or `None` when there is none.
    pub fn max_amplification(self) -> Option<usize> {
        self.amplification
    }

    /// The longest open-element stack allowed, given `synthetic` entries the
    /// parser keeps below the first real element; `None` for no limit.
    pub(crate) fn max_open(self, synthetic: usize) -> Option<usize> {
        self.depth.map(|d| d.saturating_add(synthetic))
    }

    /// The most a parse of `input` bytes may build, in the document's own
    /// measure ([`Document::approx_bytes`](crate::html::dom::Document));
    /// `None` for no limit.
    pub fn max_bytes(self, input: usize) -> Option<usize> {
        self.amplification
            .map(|r| input.saturating_mul(r).max(AMPLIFICATION_FLOOR))
    }
}

/// The guard's state across a parse.
#[derive(Default)]
pub(crate) struct Guard {
    /// The stack length above which the parse stops; `None` for no limit.
    max_open: Option<usize>,
    /// The document size above which the parse stops; `None` for no limit.
    max_bytes: Option<usize>,
    pub(crate) stopped: Option<GuardStop>,
}

impl Guard {
    /// A document parse's guard: `limit` from the root, over `input` bytes.
    pub fn for_document(limit: ParseLimits, input: usize) -> Guard {
        Guard {
            max_open: limit.max_open(0),
            max_bytes: limit.max_bytes(input),
            ..Guard::default()
        }
    }

    /// A fragment parse's guard: `limit` over the fragment's own elements,
    /// below the parser's synthetic `<html>`, over `input` bytes.
    pub fn for_fragment(limit: ParseLimits, input: usize) -> Guard {
        Guard {
            max_open: limit.max_open(1),
            max_bytes: limit.max_bytes(input),
            ..Guard::default()
        }
    }
}

impl TreeBuilder<'_> {
    /// After a token: refuse the parse when the tree is too deep or the
    /// document too large. `false` when the parse is to stop.
    pub(crate) fn guard_after(&mut self) -> bool {
        if self.guard.max_open.is_some_and(|max| self.open.len() > max) {
            self.guard.stopped = Some(GuardStop::TooDeep);
            return false;
        }
        if self
            .guard
            .max_bytes
            .is_some_and(|max| self.doc.approx_bytes() > max)
        {
            self.guard.stopped = Some(GuardStop::TooLarge);
            return false;
        }
        true
    }
}
