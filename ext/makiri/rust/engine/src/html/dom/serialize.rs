//! HTML serialization of an arena document: byte for byte what the Lexbor
//! adapter writes - Lexbor's per-node serializers (`lxb_html_serialize_cb`,
//! `lxb_html_serialize_pretty_cb`), driven by the walks the adapter drives
//! them with (`lexbor::adapter::html::serialize`), whose decisions this keeps:
//!
//! - a `<template>` writes its contents, not its own children (the pretty
//!   format keeps Lexbor's layout: the contents under a `#document-fragment`
//!   line, then the own children);
//! - text is written raw only inside an HTML-namespace `style`, `script`,
//!   `xmp`, `iframe`, `noembed`, `noframes` or `plaintext` (Lexbor's list,
//!   which also names `noscript` when scripting is on - it never is here);
//! - the walks are iterative.
//!
//! Output goes to a [`Sink`] chunk by chunk, as Lexbor's callback receives it;
//! a sink that refuses a chunk stops the walk, which then answers `Err`.

use super::handle::{HtmlAttr, HtmlElement, HtmlNode, NsId};
use super::{Document, NodeType};
use crate::ceiling_buf::{Buf, BufError};
use crate::compat::{self, Fix};
use crate::html::res::names as res;

/// Where the output goes. `false` refuses the chunk and stops the walk.
pub trait Sink {
    fn send(&mut self, bytes: &[u8]) -> bool;
}

/// The walk stopped: the sink refused a chunk, or a node of a kind Lexbor
/// does not serialize was reached (an attribute, a CDATA section, a
/// fragment written as a node).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

type R = Result<(), Stopped>;

struct Out<'s, S: Sink> {
    sink: &'s mut S,
}

impl<S: Sink> Out<'_, S> {
    #[inline]
    fn send(&mut self, b: &[u8]) -> R {
        if b.is_empty() || self.sink.send(b) {
            Ok(())
        } else {
            Err(Stopped)
        }
    }

    fn indent(&mut self, depth: usize) -> R {
        for _ in 0..depth {
            self.send(b"  ")?;
        }
        Ok(())
    }

    /// `lxb_html_serialize_send_escaping_string` (`attr` false) and
    /// `..._attribute_string` (`attr` true, which also escapes `"`).
    fn escaped(&mut self, data: &[u8], attr: bool) -> R {
        let (mut from, mut i) = (0, 0);
        while i < data.len() {
            let (rep, len): (&[u8], usize) = match data[i] {
                b'&' => (b"&amp;", 1),
                b'<' => (b"&lt;", 1),
                b'>' => (b"&gt;", 1),
                b'"' if attr => (b"&quot;", 1),
                0xC2 if data.get(i + 1) == Some(&0xA0) => (b"&nbsp;", 2),
                _ => {
                    i += 1;
                    continue;
                }
            };
            self.send(&data[from..i])?;
            self.send(rep)?;
            i += len;
            from = i;
        }
        self.send(&data[from..])
    }

    /// `lxb_html_serialize_pretty_send_escaping_string` with its indent.
    fn pretty_escaped(&mut self, data: &[u8], depth: usize) -> R {
        self.indent(depth)?;
        self.send(b"\"")?;
        let (mut from, mut i) = (0, 0);
        while i < data.len() {
            let (rep, len): (&[u8], usize) = match data[i] {
                b'&' => (b"&amp;", 1),
                b'<' => (b"&lt;", 1),
                b'>' => (b"&gt;", 1),
                0xC2 if data.get(i + 1) == Some(&0xA0) => (b"&nbsp;", 2),
                b'\n' | b'\r' => (b"\n", 1),
                _ => {
                    i += 1;
                    continue;
                }
            };
            self.send(&data[from..i])?;
            self.send(rep)?;
            if rep == b"\n" {
                self.indent(depth)?;
            }
            i += len;
            from = i;
        }
        self.send(&data[from..])?;
        self.send(b"\"")
    }

    /// `lxb_html_serialize_pretty_send_string`: raw, but every line break
    /// (kept as it is) followed by the indent. `quoted` adds the quotes the
    /// text form has; a comment has none.
    fn pretty_raw(&mut self, data: &[u8], depth: usize, quoted: bool) -> R {
        if quoted {
            self.indent(depth)?;
            self.send(b"\"")?;
        }
        let mut from = 0;
        for (i, &c) in data.iter().enumerate() {
            if c == b'\n' || c == b'\r' {
                self.send(&data[from..i])?;
                self.send(&data[i..=i])?;
                self.indent(depth)?;
                from = i + 1;
            }
        }
        self.send(&data[from..])?;
        if quoted {
            self.send(b"\"")?;
        }
        Ok(())
    }
}

/// What the walk does past a node's own markup.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Children,
    Contents,
    Void,
}

/// `lxb_html_node_is_void` by tag id in the HTML namespace, and `template`.
fn shape(n: HtmlNode<'_>) -> Shape {
    if n.ns_id() != Some(NsId::HTML) {
        return Shape::Children;
    }
    let Some(tag) = n.tag_id().and_then(|t| t.static_index()) else {
        return Shape::Children;
    };
    match tag {
        res::TAG_TEMPLATE => Shape::Contents,
        res::TAG_AREA
        | res::TAG_BASE
        | res::TAG_BASEFONT
        | res::TAG_BGSOUND
        | res::TAG_BR
        | res::TAG_COL
        | res::TAG_EMBED
        | res::TAG_FRAME
        | res::TAG_HR
        | res::TAG_IMG
        | res::TAG_INPUT
        | res::TAG_KEYGEN
        | res::TAG_LINK
        | res::TAG_META
        | res::TAG_PARAM
        | res::TAG_SOURCE
        | res::TAG_TRACK
        | res::TAG_WBR => Shape::Void,
        _ => Shape::Children,
    }
}

/// Whether a text node is written raw: its parent is an HTML-namespace
/// element of Lexbor's raw-text list.
fn raw_text(n: HtmlNode<'_>) -> bool {
    let Some(parent) = n.parent() else {
        return false;
    };
    if parent.ns_id() != Some(NsId::HTML) {
        return false;
    }
    matches!(
        parent.tag_id().and_then(|t| t.static_index()),
        Some(
            res::TAG_STYLE
                | res::TAG_SCRIPT
                | res::TAG_XMP
                | res::TAG_IFRAME
                | res::TAG_NOEMBED
                | res::TAG_NOFRAMES
                | res::TAG_PLAINTEXT
        )
    )
}

/// `lxb_html_serialize_attribute_cb` / `..._pretty_attribute_cb` (no
/// option): the name by the namespace Lexbor stores on the attribute node,
/// then the escaped value. The Standard's name goes by the attribute's own
/// namespace and its case-preserved local name (R3): Lexbor's node
/// namespace is the element's for an attribute in none, and its stored
/// local name lower case, which differ only for an attribute the DOM made -
/// `viewBox` set on an element in no namespace was written `viewbox`.
fn attribute<S: Sink>(out: &mut Out<'_, S>, a: HtmlAttr<'_>) -> R {
    let (local, ns) = if compat::fixed(Fix::R3) {
        (a.dom_local_name(), a.own_ns())
    } else {
        (a.local_name(), a.node().ns_id())
    };
    match ns {
        None => out.send(local)?,
        Some(NsId::XML) => {
            out.send(b"xml:")?;
            out.send(local)?;
        }
        Some(NsId::XMLNS) => {
            if local == b"xmlns" {
                out.send(b"xmlns")?;
            } else {
                out.send(b"xmlns:")?;
                out.send(local)?;
            }
        }
        Some(NsId::XLINK) => {
            out.send(b"xlink:")?;
            out.send(local)?;
        }
        Some(_) => out.send(a.qualified_name())?,
    }
    out.send(b"=\"")?;
    out.escaped(a.value(), true)?;
    out.send(b"\"")
}

/// The name an element's tags are written with: the Standard's local name
/// for an element in the HTML, SVG or MathML namespace, the qualified name
/// for any other. Lexbor wrote the qualified name for all, which differs
/// only where a prefix was given (`create_element_ns`): the parser makes
/// none.
fn tag_name(el: HtmlElement<'_>) -> &[u8] {
    if el.has_written_name()
        && matches!(el.node().ns_id(), Some(NsId::HTML | NsId::SVG | NsId::MATH))
        && compat::fixed(Fix::R2)
    {
        return el.dom_local_name();
    }
    el.qualified_name()
}

/// `lxb_html_serialize_element_cb`: the start tag.
fn start_tag<S: Sink>(out: &mut Out<'_, S>, el: HtmlElement<'_>) -> R {
    out.send(b"<")?;
    out.send(tag_name(el))?;
    for a in el.attrs() {
        out.send(b" ")?;
        attribute(out, a)?;
    }
    out.send(b">")
}

/// `lxb_html_serialize_cb`: one node's own markup.
fn node_markup<S: Sink>(out: &mut Out<'_, S>, n: HtmlNode<'_>) -> R {
    match n.node_type() {
        NodeType::Element => start_tag(out, n.element().ok_or(Stopped)?),
        NodeType::Text => {
            let data = n.data().unwrap_or_default();
            if raw_text(n) {
                out.send(data)
            } else {
                out.escaped(data, false)
            }
        }
        NodeType::Comment => {
            out.send(b"<!--")?;
            out.send(n.data().unwrap_or_default())?;
            out.send(b"-->")
        }
        NodeType::Pi => {
            out.send(b"<?")?;
            out.send(n.pi_target().unwrap_or_default())?;
            out.send(b" ")?;
            out.send(n.data().unwrap_or_default())?;
            out.send(b"?>")
        }
        NodeType::DocumentType => {
            out.send(b"<!DOCTYPE ")?;
            out.send(n.node_name())?;
            out.send(b">")
        }
        NodeType::Document => out.send(b"<#document>"),
        _ => Err(Stopped),
    }
}

/// `lxb_html_serialize_pretty_cb` at `depth`, with no option: one node's
/// line.
fn pretty_markup<S: Sink>(out: &mut Out<'_, S>, n: HtmlNode<'_>, depth: usize) -> R {
    match n.node_type() {
        NodeType::Text => {
            let data = n.data().unwrap_or_default();
            if raw_text(n) {
                out.pretty_raw(data, depth, true)?;
            } else {
                out.pretty_escaped(data, depth)?;
            }
        }
        NodeType::Comment => {
            out.indent(depth)?;
            out.send(b"<!-- ")?;
            out.pretty_raw(n.data().unwrap_or_default(), depth, false)?;
            out.send(b" -->")?;
        }
        NodeType::Document => {
            out.indent(depth)?;
            out.send(b"#document")?;
        }
        NodeType::Element | NodeType::Pi | NodeType::DocumentType => {
            out.indent(depth)?;
            node_markup(out, n)?;
        }
        _ => return Err(Stopped),
    }
    out.send(b"\n")
}

/// `lxb_html_serialize_element_closed_cb` for an element that is not void;
/// nothing for any other node.
fn end_tag<S: Sink>(out: &mut Out<'_, S>, n: HtmlNode<'_>) -> R {
    let Some(el) = n.element() else {
        return Ok(());
    };
    if shape(n) == Shape::Void {
        return Ok(());
    }
    out.send(b"</")?;
    out.send(tag_name(el))?;
    out.send(b">")
}

/// What of a node is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The node and its subtree (`outerHTML`); a Document as its children.
    Outer,
    /// Its children only (`innerHTML`): in [`Layout::Plain`], a template's
    /// contents.
    Inner,
}

/// How it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// As the HTML Standard serializes.
    Plain,
    /// Lexbor's pretty format, at indent 0.
    Pretty,
}

/// Serialize `node` into `sink`, `scope` of it laid out as `layout`.
pub fn write<S: Sink>(node: HtmlNode<'_>, scope: Scope, layout: Layout, sink: &mut S) -> R {
    let mut out = Out { sink };
    let outer = scope == Scope::Outer && node.node_type() != NodeType::Document;
    if layout == Layout::Pretty {
        if outer {
            return pretty_subtree(&mut out, node);
        }
        let mut c = node.first_child();
        while let Some(n) = c {
            pretty_subtree(&mut out, n)?;
            c = n.next();
        }
        return Ok(());
    }
    if outer {
        return subtree(&mut out, node);
    }
    /* The Standard's fragment serializing, step 1: a void element's
     * children, which only the DOM can give it, are not written. */
    if shape(node) == Shape::Void && compat::fixed(Fix::R1) {
        return Ok(());
    }
    let mut c = match node.template_content() {
        Some(contents) => contents.first_child(),
        None => node.first_child(),
    };
    while let Some(n) = c {
        subtree(&mut out, n)?;
        c = n.next();
    }
    Ok(())
}

/// `root` and its subtree, a template's contents in place of its children.
fn subtree<S: Sink>(out: &mut Out<'_, S>, root: HtmlNode<'_>) -> R {
    let mut node = root;
    loop {
        node_markup(out, node)?;
        let first = match shape(node) {
            Shape::Children => node.first_child(),
            Shape::Contents => node.template_content().and_then(HtmlNode::first_child),
            Shape::Void => None,
        };
        if let Some(c) = first {
            node = c;
            continue;
        }
        loop {
            end_tag(out, node)?;
            if node == root {
                return Ok(());
            }
            if let Some(n) = node.next() {
                node = n;
                break;
            }
            node = match node.parent().or_else(|| node.template_host()) {
                Some(p) => p,
                None => return Ok(()),
            };
        }
    }
}

/// The pretty format of `root` and its subtree at indent 0.
fn pretty_subtree<S: Sink>(out: &mut Out<'_, S>, root: HtmlNode<'_>) -> R {
    let mut node = root;
    let mut depth = 0usize;
    'write: loop {
        pretty_markup(out, node, depth)?;
        if let Some(first) = node.template_content().and_then(HtmlNode::first_child) {
            out.indent(depth + 1)?;
            out.send(b"#document-fragment\n")?;
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
        loop {
            if node.element().is_some() && shape(node) != Shape::Void {
                out.indent(depth)?;
                end_tag(out, node)?;
                out.send(b"\n")?;
            }
            if node == root {
                return Ok(());
            }
            if let Some(n) = node.next() {
                node = n;
                continue 'write;
            }
            let Some(parent) = node.parent() else {
                return Ok(());
            };
            match parent.template_host() {
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

/* ---------- into a buffer ---------- */

/// Every document's ceiling is at least this.
const CEILING_FLOOR: usize = 65536;

/// The output ceiling for `doc`, as the Lexbor serializer's: 32x the bytes
/// the document holds, plus every node writing the longest name four times,
/// over [`CEILING_FLOOR`] - so a legitimate document round-trips and a
/// pathological pretty-print fails closed.
fn ceiling(doc: &Document) -> usize {
    let longest = doc.longest_name();
    CEILING_FLOOR
        .saturating_add(doc.owned_bytes().saturating_mul(32))
        .saturating_add(doc.node_count().saturating_mul(longest.saturating_mul(4)))
}

struct BufSink(Buf);

impl Sink for BufSink {
    #[inline]
    fn send(&mut self, bytes: &[u8]) -> bool {
        !matches!(self.0.append(bytes), Err(BufError::Oom | BufError::Limit))
    }
}

/// Serialize `node` into owned UTF-8 bytes - see [`write`]. `None` when
/// the output could not be held (out of memory, or past the ceiling) or a
/// node Lexbor does not serialize was reached.
pub fn serialize(node: HtmlNode<'_>, scope: Scope, layout: Layout) -> Option<Buf> {
    let doc = node.owner_document().arena();
    let mut buf = Buf::new(ceiling(doc));
    let whole = node.node_type() == NodeType::Document
        || node.owner_document().as_node().document_root() == Some(node);
    if whole {
        /* Best-effort pre-size: markup is about the size of the text the
         * document stores, plus a few bytes per node. */
        let _ = buf.reserve(doc.stored_bytes() + doc.node_count() * 8);
    }
    let mut sink = BufSink(buf);
    write(node, scope, layout, &mut sink).ok()?;
    Some(sink.0)
}
