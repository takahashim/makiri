//! The HTML serializer's walk: Lexbor's per-node serializers
//! (`lxb_html_serialize_cb`, and `lxb_html_serialize_pretty_cb` for the
//! pretty format), driven from here rather than by Lexbor's own loops.
//!
//! Three things are decided here instead of there:
//!
//! - "Serializing HTML fragments" takes a template element's template
//!   CONTENTS in place of its children. Lexbor writes the contents and then
//!   walks into the template's own children as it does any element's, so a
//!   template given a child with `appendChild` (`add_child` here) wrote it
//!   too - where a browser writes the contents alone. (The pretty format is
//!   Lexbor's own, not the Standard's, and keeps Lexbor's layout: contents
//!   under a `#document-fragment` line, then the own children.)
//! - Which text is written unescaped follows the Standard's rule, checked
//!   here ([`escaped_here`]): only a child of one of the HTML elements it
//!   names (`style`, `script`, ...).
//! - Iterative, and a template's contents are walked as its children rather
//!   than by a nested call, so neither a deep tree nor deeply nested templates
//!   grow the native stack.
//!
//! Everything else is Lexbor's: each node's own markup, and an end tag is
//! exactly what its (non-exported) `lxb_html_serialize_element_closed_cb`
//! writes.

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

/// The data of a text node this walk writes itself, escaped: one whose parent
/// has the local name of an element the Standard writes text raw inside
/// (`style`, `script`, ...) but is not in the HTML namespace, which that rule
/// is for. `None` for every other node, which Lexbor writes.
fn escaped_here(n: HtmlNode<'_>) -> Option<&[u8]> {
    use lxb::*;
    if n.node_type() != NodeType::Text {
        return None;
    }
    let parent = n.tree_parent()?;
    // SAFETY: a live node; two plain field reads.
    let (tag, ns) = unsafe { ((*parent.as_raw()).local_name, (*parent.as_raw()).ns) };
    if ns == lxb_ns_id_enum_t_LXB_NS_HTML as usize {
        return None;
    }
    #[allow(non_upper_case_globals)]
    let raw_name = matches!(
        tag as u32,
        lxb_tag_id_enum_t_LXB_TAG_STYLE
            | lxb_tag_id_enum_t_LXB_TAG_SCRIPT
            | lxb_tag_id_enum_t_LXB_TAG_XMP
            | lxb_tag_id_enum_t_LXB_TAG_IFRAME
            | lxb_tag_id_enum_t_LXB_TAG_NOEMBED
            | lxb_tag_id_enum_t_LXB_TAG_NOFRAMES
            | lxb_tag_id_enum_t_LXB_TAG_PLAINTEXT
            | lxb_tag_id_enum_t_LXB_TAG_NOSCRIPT
    );
    raw_name.then(|| n.char_data().unwrap_or_default())
}

/// The chunk writer the walks share: `cb` with `ctx`, Lexbor's status back.
struct Out {
    cb: unsafe extern "C" fn(*const u8, usize, *mut core::ffi::c_void) -> lxb::lxb_status_t,
    ctx: *mut core::ffi::c_void,
}

impl Out {
    /// `None` when there is no callback, which Lexbor's serializers do not
    /// accept either.
    fn new(cb: lxb::lxb_html_serialize_cb_f, ctx: *mut core::ffi::c_void) -> Option<Out> {
        Some(Out { cb: cb?, ctx })
    }

    fn send(&self, bytes: &[u8]) -> lxb::lxb_status_t {
        if bytes.is_empty() {
            return OK;
        }
        // SAFETY: the callback the caller of the walk vouched for, called as
        // Lexbor calls it.
        unsafe { (self.cb)(bytes.as_ptr(), bytes.len(), self.ctx) }
    }

    fn indent(&self, depth: usize) -> lxb::lxb_status_t {
        for _ in 0..depth {
            let st = self.send(b"  ");
            if st != OK {
                return st;
            }
        }
        OK
    }

    /// `data` escaped as the Standard escapes text - `&`, U+00A0, `<`, `>` -
    /// which is what Lexbor writes for any text it does not take as raw.
    /// With `pretty`, in Lexbor's pretty layout at that depth: quoted, and
    /// every line break followed by the indent.
    fn escaped_text(&self, data: &[u8], pretty: Option<usize>) -> lxb::lxb_status_t {
        macro_rules! send {
            ($b:expr) => {{
                let st = self.send($b);
                if st != OK {
                    return st;
                }
            }};
        }
        if let Some(depth) = pretty {
            let st = self.indent(depth);
            if st != OK {
                return st;
            }
            send!(b"\"");
        }
        let mut from = 0;
        let mut i = 0;
        while i < data.len() {
            let (rep, len): (&[u8], usize) = match data[i] {
                b'&' => (b"&amp;", 1),
                b'<' => (b"&lt;", 1),
                b'>' => (b"&gt;", 1),
                0xC2 if data.get(i + 1) == Some(&0xA0) => (b"&nbsp;", 2),
                b'\n' | b'\r' if pretty.is_some() => (b"\n", 1),
                _ => {
                    i += 1;
                    continue;
                }
            };
            send!(&data[from..i]);
            send!(rep);
            if let (Some(depth), b"\n") = (pretty, rep) {
                let st = self.indent(depth);
                if st != OK {
                    return st;
                }
            }
            i += len;
            from = i;
        }
        send!(&data[from..]);
        if pretty.is_some() {
            send!(b"\"\n");
        }
        OK
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
        let Some(out) = Out::new(cb, ctx) else {
            return lxb::consts::STATUS_ERROR as lxb::lxb_status_t;
        };
        let root = self;
        let mut node = root;
        loop {
            let st = match escaped_here(node) {
                Some(data) => out.escaped_text(data, None),
                // SAFETY: a live node of a live document; `cb` is the caller's.
                None => unsafe { lxb::lxb_html_serialize_cb(node.as_raw(), cb, ctx) },
            };
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

    /// The pretty format, as `lxb_html_serialize_pretty_tree_cb` writes it
    /// (`deep` false: a Document as its children, any other node with its
    /// subtree) or `lxb_html_serialize_pretty_deep_cb` (`deep`: the node's own
    /// children), at indent 0 - but walked here, without native recursion, and
    /// with the text [`escaped_here`] picks escaped.
    ///
    /// # Safety
    /// As [`serialize_to`](Self::serialize_to).
    pub(in crate::lexbor) unsafe fn serialize_pretty_to(
        self,
        deep: bool,
        cb: lxb::lxb_html_serialize_cb_f,
        ctx: *mut core::ffi::c_void,
    ) -> lxb::lxb_status_t {
        let Some(out) = Out::new(cb, ctx) else {
            return lxb::consts::STATUS_ERROR as lxb::lxb_status_t;
        };
        if !deep && self.node_type() != NodeType::Document {
            // SAFETY: the caller's contract.
            return unsafe { self.pretty_subtree(&out, cb, ctx) };
        }
        let mut c = self.first_child();
        while let Some(n) = c {
            // SAFETY: the caller's contract.
            let st = unsafe { n.pretty_subtree(&out, cb, ctx) };
            if st != OK {
                return st;
            }
            c = n.next();
        }
        OK
    }

    /// `lxb_html_serialize_pretty_node_cb` for `self` at indent 0: each node's
    /// line, a template's contents under a `#document-fragment` line two
    /// levels in, then its own children, and an end-tag line per element that
    /// is not void.
    ///
    /// # Safety
    /// As [`serialize_to`](Self::serialize_to).
    unsafe fn pretty_subtree(
        self,
        out: &Out,
        cb: lxb::lxb_html_serialize_cb_f,
        ctx: *mut core::ffi::c_void,
    ) -> lxb::lxb_status_t {
        macro_rules! check {
            ($e:expr) => {{
                let st = $e;
                if st != OK {
                    return st;
                }
            }};
        }
        let opt = lxb::lxb_html_serialize_opt_LXB_HTML_SERIALIZE_OPT_UNDEF as _;
        let root = self;
        let mut node = root;
        let mut depth = 0usize;
        'write: loop {
            match escaped_here(node) {
                Some(data) => check!(out.escaped_text(data, Some(depth))),
                // SAFETY: a live node of a live document; `cb` is the caller's.
                None => check!(unsafe {
                    lxb::lxb_html_serialize_pretty_cb(node.as_raw(), opt, depth, cb, ctx)
                }),
            }
            if let Some(first) = node.template_content().and_then(HtmlNode::first_child) {
                check!(out.indent(depth + 1));
                check!(out.send(b"#document-fragment\n"));
                node = first;
                depth += 2;
                continue 'write;
            }
            if shape(node) != Shape::Void {
                if let Some(first) = node.first_child() {
                    node = first;
                    depth += 1;
                    continue 'write;
                }
            }
            /* Close what ends here, and every ancestor it was the last of. */
            loop {
                if node.element().is_some() && shape(node) != Shape::Void {
                    check!(out.indent(depth));
                    // SAFETY: as above.
                    check!(unsafe { node.write_end_tag(cb, ctx) });
                    check!(out.send(b"\n"));
                }
                if node == root {
                    return OK;
                }
                if let Some(n) = node.next() {
                    node = n;
                    continue 'write;
                }
                let Some(parent) = node.tree_parent() else {
                    return OK;
                };
                match parent.template_host() {
                    /* Out of a template's contents: on to its own children,
                     * one level in, or else its end tag. */
                    Some(template) => {
                        depth = depth.saturating_sub(2);
                        node = template;
                        if let Some(first) = template.first_child() {
                            node = first;
                            depth += 1;
                            continue 'write;
                        }
                    }
                    None => {
                        depth = depth.saturating_sub(1);
                        node = parent;
                    }
                }
            }
        }
    }
}
