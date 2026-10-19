//! Source-location tracking.
//!
//! Lexbor does not record where in the input a node came from, and we stay on
//! vanilla Lexbor, so it is taken from the tokenizer instead:
//!
//! 1. [`Stamper`] rides the tokenizer's token-done hook
//!    (`tree_guard::TokenHook`, which also carries the tree-depth guard). For
//!    each element start tag it notes the byte offset before the tree builder
//!    runs, and [`stamp_created`] writes it onto the element the tree builder
//!    created for that tag - at creation, so no later matching is needed.
//! 2. [`lines_build`] maps a byte offset to a 1-based line for `Node#line`.
//!
//! An element that cannot be told apart for certain as the product of its own
//! start tag is left UNSTAMPED - `#line` answers nil - rather than given a
//! wrong location. That covers every element the parser invents.
//!
//! # `node.user` is reserved
//!
//! CLAUDE.md reserves Lexbor's `node.user` for exactly this offset. The field and
//! its encoding belong to `html` (`HtmlNode::stamp_source_offset` /
//! `source_offset`); this module decides WHICH offset an element gets.
//!
//! # The stamping runs inside Lexbor
//!
//! Both halves are called from the tokenizer's hook, from C, once per token.
//! They must not unwind and must not stop the parse. Both are structural: the
//! hook calls them under a panic latch, and neither has a failure path - a
//! doubtful case only leaves the element unstamped.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::lexbor::abi as lxb;

type Token = lxb::lxb_html_token_t;

use super::html::{HtmlNode, TagId, TAG_EM_DOCTYPE};
const TOKEN_TYPE_CLOSE: i32 = lxb::lxb_html_token_type_LXB_HTML_TOKEN_TYPE_CLOSE as i32;

/* The line table is `crate::source_lines`'s, shared with the arena DOM. */
pub use crate::source_lines::{lines_build, Lines};

/* ------------------------------------------------------------------ *
 * stamping at creation                                               *
 * ------------------------------------------------------------------ */

/// Stamps each element with the offset of the start tag that created it, as
/// the parse runs. `tree_guard::TokenHook` calls
/// [`start_tag`](Self::start_tag) before the tree builder sees a token and
/// [`stamp_created`] after it.
///
/// Holds a pointer to the input for the parse only: the hook it lives in is
/// gone when the parse returns, and what it wrote are plain offsets.
pub struct Stamper {
    /// The start of the input buffer, which offsets are relative to.
    first: *const u8,
    len: usize,
}

/// An element start tag the tree builder is about to process.
#[derive(Clone, Copy)]
pub struct StartTag {
    tag_id: TagId,
    offset: usize,
}

/// The tree just before the tree builder ran: the current node, and the last
/// child of the place it inserts into.
#[derive(Clone, Copy)]
pub struct Before<'doc> {
    current: Option<HtmlNode<'doc>>,
    last_child: Option<HtmlNode<'doc>>,
}

impl<'doc> Before<'doc> {
    /// The snapshot, given the current node.
    #[inline]
    pub fn at(current: Option<HtmlNode<'doc>>) -> Before<'doc> {
        Before {
            current,
            last_child: current.and_then(|c| inserts_into(c).last_child()),
        }
    }
}

/// Where the tree builder appends a child of `node`: an HTML `<template>`'s
/// contents fragment, otherwise the node itself.
#[inline]
fn inserts_into(node: HtmlNode<'_>) -> HtmlNode<'_> {
    node.template_content().unwrap_or(node)
}

impl Stamper {
    /// A stamper for the tokens of `src`.
    pub fn new(src: &[u8]) -> Stamper {
        Stamper {
            first: src.as_ptr(),
            len: src.len(),
        }
    }

    /// The token as an element start tag, or None.
    ///
    /// The special tag ids (text, comment, doctype, document, eof) sit at or
    /// below `LXB_TAG__EM_DOCTYPE` and are skipped, as are end-tags. A void or
    /// self-closing start tag (`CLOSE_SELF`) leaves the CLOSE bit clear, so it
    /// IS one. A `begin` outside the input - which a single-chunk parse never
    /// hands out - gives None rather than an offset into something else.
    ///
    /// # Safety
    /// `token` must be the live token the tokenizer is handing out.
    #[inline]
    pub unsafe fn start_tag(&self, token: *const Token) -> Option<StartTag> {
        if (*token).tag_id <= TAG_EM_DOCTYPE || ((*token).type_ & TOKEN_TYPE_CLOSE) != 0 {
            return None;
        }
        let offset = ((*token).begin as usize).checked_sub(self.first as usize)?;
        if offset >= self.len {
            return None;
        }
        Some(StartTag {
            /* Above the special ids, so never UNDEF. */
            tag_id: TagId::from_raw((*token).tag_id)?,
            offset,
        })
    }
}

/// Stamp the element the tree builder just created for `tag`, when it can be
/// told apart for certain; otherwise stamp nothing.
///
/// `now` is the current node after the tree builder ran. The element created
/// for a start tag is inserted LAST, after anything the parser invents on the
/// way (an implied `<tbody>`, reconstructed formatting elements), so it is
/// either the new current node or - a void or self-closing element, pushed and
/// popped at once - the last child of where the current node inserts. That
/// candidate is stamped only if it is an element of the token's tag, carries
/// no stamp, has no children yet, and is neither the node that was current
/// before nor the last child that place already had. A node that existed
/// before this token is one of those two, holds children (it is an ancestor of
/// what was current), or was stamped by its own start tag.
///
/// Everything else stays unstamped and answers nil: the elements the parser
/// invents (implied html/head/body/tbody/colgroup, formatting elements the
/// adoption agency or reconstruction recreates), a start tag the tree builder
/// ignores or merges (a second `<body>`), and a void element fostered out of a
/// table, which lands before the table rather than at either place.
#[inline]
pub fn stamp_created(tag: StartTag, before: Before<'_>, now: Option<HtmlNode<'_>>) {
    let Some(now) = now else {
        return;
    };
    let candidate = if Some(now) != before.current && now.tag_id() == Some(tag.tag_id) {
        Some(now)
    } else {
        inserts_into(now).last_child()
    };
    let Some(x) = candidate else {
        return;
    };
    if Some(x) == before.current
        || Some(x) == before.last_child
        || x.element().is_none()
        || x.tag_id() != Some(tag.tag_id)
        || x.source_offset().is_some()
        || x.first_child().is_some()
    {
        return;
    }
    x.stamp_source_offset(tag.offset);
}
