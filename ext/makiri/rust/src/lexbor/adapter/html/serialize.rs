//! The HTML serializer's walk: Lexbor's `lxb_html_serialize_node_cb`, driven
//! from here rather than by Lexbor, so a `<template>` is written as the HTML
//! Standard writes it.
//!
//! "Serializing HTML fragments" takes a template element's template CONTENTS
//! in place of its children. Lexbor writes the contents and then walks into
//! the template's own children as it does any element's, so a template given
//! a child with `appendChild` (`add_child` here) wrote it too - where a
//! browser writes the contents alone. Everything else is Lexbor's: each
//! node's own markup is `lxb_html_serialize_cb`, and an end tag is exactly
//! what its (non-exported) `lxb_html_serialize_element_closed_cb` writes.
//!
//! Iterative, like Lexbor's loop, and a template's contents are walked as its
//! children rather than by a nested call (Lexbor's `deep_cb`), so neither a
//! deep tree nor deeply nested templates grow the native stack.

#![allow(unsafe_code)]

use super::*;

/// Lexbor's status for a finished write.
const OK: lxb::lxb_status_t = lxb::consts::STATUS_OK as lxb::lxb_status_t;

/// What the walk does with a node past its own markup: descend into its
/// children, into a template's contents, or nowhere (a void element).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Children,
    Contents,
    Void,
}

/// The void elements `lxb_html_node_is_void` lists - by tag id, in the HTML
/// namespace only - and `template`, read from the node in one go. A Lexbor
/// inline in C, so called through its `_noi` twin it was an FFI call per node,
/// twice for an element, which cost `to_html` ~10%.
fn shape(n: HtmlNode<'_>) -> Shape {
    use lxb::*;
    // SAFETY: a live node; two plain field reads.
    let (tag, ns) = unsafe { ((*n.as_raw()).local_name, (*n.as_raw()).ns) };
    if ns != lxb_ns_id_enum_t_LXB_NS_HTML as usize {
        return Shape::Children;
    }
    #[allow(non_upper_case_globals)]
    match tag as u32 {
        lxb_tag_id_enum_t_LXB_TAG_TEMPLATE => Shape::Contents,
        lxb_tag_id_enum_t_LXB_TAG_AREA
        | lxb_tag_id_enum_t_LXB_TAG_BASE
        | lxb_tag_id_enum_t_LXB_TAG_BASEFONT
        | lxb_tag_id_enum_t_LXB_TAG_BGSOUND
        | lxb_tag_id_enum_t_LXB_TAG_BR
        | lxb_tag_id_enum_t_LXB_TAG_COL
        | lxb_tag_id_enum_t_LXB_TAG_EMBED
        | lxb_tag_id_enum_t_LXB_TAG_FRAME
        | lxb_tag_id_enum_t_LXB_TAG_HR
        | lxb_tag_id_enum_t_LXB_TAG_IMG
        | lxb_tag_id_enum_t_LXB_TAG_INPUT
        | lxb_tag_id_enum_t_LXB_TAG_KEYGEN
        | lxb_tag_id_enum_t_LXB_TAG_LINK
        | lxb_tag_id_enum_t_LXB_TAG_META
        | lxb_tag_id_enum_t_LXB_TAG_PARAM
        | lxb_tag_id_enum_t_LXB_TAG_SOURCE
        | lxb_tag_id_enum_t_LXB_TAG_TRACK
        | lxb_tag_id_enum_t_LXB_TAG_WBR => Shape::Void,
        _ => Shape::Children,
    }
}

impl<'doc> HtmlNode<'doc> {
    /// Serialize this node with its subtree through `cb` - or, with `deep`,
    /// its children only - as `lxb_html_serialize_tree_cb` /
    /// `lxb_html_serialize_deep_cb` do, but with a `<template>` written as
    /// its contents only (see the module doc). A Document, like Lexbor's tree
    /// serializer, is written as its children. Returns the first non-OK
    /// status a write gave, or OK.
    ///
    /// # Safety
    /// `cb` must accept every chunk with `ctx`, as Lexbor's own serializers
    /// require; the tree must not change during the call.
    pub(in crate::lexbor) unsafe fn serialize_to(
        self,
        deep: bool,
        cb: lxb::lxb_html_serialize_cb_f,
        ctx: *mut core::ffi::c_void,
    ) -> lxb::lxb_status_t {
        if !deep && self.node_type() != NodeType::Document {
            // SAFETY: the caller's contract.
            return unsafe { self.serialize_subtree(cb, ctx) };
        }
        /* The entry's children - a template's contents - even when it is
         * void, as `deep_cb` takes them. */
        let mut c = match self.template_content() {
            Some(contents) => contents.first_child(),
            None => self.first_child(),
        };
        while let Some(n) = c {
            // SAFETY: the caller's contract.
            let st = unsafe { n.serialize_subtree(cb, ctx) };
            if st != OK {
                return st;
            }
            c = n.next();
        }
        OK
    }

    /// `lxb_html_serialize_node_cb` for `root`, with a template's contents in
    /// place of its children.
    ///
    /// # Safety
    /// As [`serialize_to`](Self::serialize_to).
    unsafe fn serialize_subtree(
        self,
        cb: lxb::lxb_html_serialize_cb_f,
        ctx: *mut core::ffi::c_void,
    ) -> lxb::lxb_status_t {
        let root = self;
        let mut node = root;
        loop {
            // SAFETY: a live node of a live document; `cb` is the caller's.
            let st = unsafe { lxb::lxb_html_serialize_cb(node.as_raw(), cb, ctx) };
            if st != OK {
                return st;
            }
            let first = match shape(node) {
                Shape::Children => node.first_child(),
                Shape::Contents => node.template_content().and_then(HtmlNode::first_child),
                Shape::Void => None,
            };
            if let Some(c) = first {
                node = c;
                continue;
            }
            /* Close what ends here, and every ancestor it was the last of. */
            loop {
                // SAFETY: as above.
                let st = unsafe { node.write_end_tag(cb, ctx) };
                if st != OK {
                    return st;
                }
                if node == root {
                    return OK;
                }
                if let Some(n) = node.next() {
                    node = n;
                    break;
                }
                /* Out of a template's contents, the template is next: the
                 * fragment has no parent, and is written as nothing. */
                node = match node.tree_parent().or_else(|| node.template_host()) {
                    Some(p) => p,
                    None => return OK,
                };
            }
        }
    }

    /// The end tag `lxb_html_serialize_element_closed_cb` writes - `</`, the
    /// qualified name, `>` - for an element that is not void; nothing for
    /// any other node.
    ///
    /// # Safety
    /// As [`serialize_to`](Self::serialize_to).
    unsafe fn write_end_tag(
        self,
        cb: lxb::lxb_html_serialize_cb_f,
        ctx: *mut core::ffi::c_void,
    ) -> lxb::lxb_status_t {
        let Some(el) = self.element() else {
            return OK;
        };
        if shape(self) == Shape::Void {
            return OK;
        }
        let Some(send) = cb else {
            return lxb::consts::STATUS_ERROR as lxb::lxb_status_t;
        };
        for part in [&b"</"[..], el.qualified_name(), &b">"[..]] {
            // SAFETY: `cb` is the caller's, called as Lexbor calls it.
            let st = unsafe { send(part.as_ptr(), part.len(), ctx) };
            if st != OK {
                return st;
            }
        }
        OK
    }
}
