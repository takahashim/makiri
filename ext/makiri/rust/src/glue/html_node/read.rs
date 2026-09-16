//! The HTML node's readers: name and namespace, the DTD identifiers, content,
//! navigation, attributes, source line and document order.
//!
//! Every read of the Lexbor tree goes through the typed handles of
//! `dom_adapter::html`, which are safe to use; what stays `unsafe` here is the
//! Ruby side - wrapping a node into an object, pushing onto a NodeSet, and the
//! per-document indexes.
//!
//! # Where a GC may run
//!
//! Building any String or NodeSet is a GC point, so a view borrowed from a Ruby
//! String must not be live across one. The attribute lookups take their name
//! through `ruby_verified_text`, whose guard keeps the String reachable until
//! it drops, and read the bytes before building anything. Bytes borrowed from
//! the document are arena memory, which a GC does not move.

#![allow(unsafe_code)]

use core::ffi::c_char;

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, Ruby, Value};

use super::ty;
use super::{arg_node, wrap, wrap_node};
use crate::dom_adapter::html::HtmlNode;
use crate::glue::abi::{
    doc_parsed, error_class, is_kind_of, node_set_new, node_set_push, ruby_str_from_borrowed,
    ruby_str_from_slices, ruby_verified_text, LxbAttr, CLASS_NODE, CLASS_XML_DOCUMENT,
};
use crate::text::BorrowedText;

/* ------------------------------------------------------------------ *
 * small helpers                                                      *
 * ------------------------------------------------------------------ */

/// A UTF-8 String copied from bytes the document lends.
fn dom_str(bytes: &[u8]) -> Value {
    // SAFETY: the String copies the bytes. They are valid UTF-8 whenever they
    // come from the document, by the text-input contract; bytes that were not
    // would make a broken String, not a memory error.
    unsafe {
        Value::from_raw(ruby_str_from_borrowed(BorrowedText::from_raw_parts(
            bytes.as_ptr() as *const c_char,
            bytes.len(),
        )))
    }
}

fn nil(ruby: &Ruby) -> Value {
    ruby.qnil().as_value()
}

/// An Element's or Attribute's qualified name with its local name, or `None`
/// for any other kind. The one place the element-vs-attribute accessor pair is
/// chosen.
fn qname(node: HtmlNode<'_>) -> Option<(&[u8], &[u8])> {
    if let Some(el) = node.element() {
        return Some((el.qualified_name(), el.local_name()));
    }
    node.attr().map(|at| (at.qualified_name(), at.local_name()))
}

/// The prefix of a qualified name whose local part is `local_len` bytes
/// (qualified is `prefix:local` or a bare `local`).
///
/// Centralises the `len`-vs-`local_len + 1` boundary so [`prefix`] and
/// [`namespace_uri`] cannot drift apart on it, and so a colon inside a local
/// name is never mistaken for the separator.
fn qname_prefix(q: &[u8], local_len: usize) -> Option<&[u8]> {
    (q.len() > local_len + 1).then(|| &q[..q.len() - local_len - 1])
}

/// A node's namespace URI as a String, or nil. The one place an lxb ns-id
/// becomes a Ruby URI.
fn ns_uri_str(ruby: &Ruby, node: HtmlNode<'_>) -> Value {
    node.ns_uri().map_or_else(|| nil(ruby), dom_str)
}

/// The fixed namespaces the HTML parser assigns to foreign-content attributes by
/// prefix (the "adjust foreign attributes" step).
///
/// Lexbor tags an attribute node with its ELEMENT's ns rather than the
/// attribute's own, so a parsed attribute's namespaceURI is resolved from its
/// prefix here rather than from `node.ns`.
fn attr_ns_for_prefix(p: &[u8]) -> Option<&'static str> {
    match p {
        b"xlink" => Some("http://www.w3.org/1999/xlink"),
        b"xml" => Some("http://www.w3.org/XML/1998/namespace"),
        b"xmlns" => Some("http://www.w3.org/2000/xmlns/"),
        _ => None,
    }
}

/* ------------------------------------------------------------------ *
 * name / type / content                                              *
 * ------------------------------------------------------------------ */

/// `#name`. Matches Nokogiri: the lowercase tag name for an HTML element
/// (Lexbor lowercases during tokenization), and the un-prefixed DOM names
/// `text` / `comment` / `#cdata-section` / `document` for the other kinds.
pub fn name(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let node = this.node();
    if let Some(el) = node.element() {
        return dom_str(el.qualified_name());
    }
    if let Some(at) = node.attr() {
        return dom_str(at.qualified_name());
    }
    match node.node_type() {
        ty::TEXT => ruby.str_new("text").as_value(),
        ty::COMMENT => ruby.str_new("comment").as_value(),
        ty::CDATA => ruby.str_new("#cdata-section").as_value(),
        ty::DOCUMENT => ruby.str_new("document").as_value(),
        _ => dom_str(node.node_name()),
    }
}

/// `#local_name` (DOM `localName`): the name without any prefix - `div` for
/// `<div>`, `path` for an SVG `<path>`, `href` for an `xlink:href` attribute.
/// Element and Attribute only; the DOM gives a Text/Comment/Document none.
pub fn local_name(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let node = this.node();
    if let Some(el) = node.element() {
        return dom_str(el.local_name());
    }
    let Some(at) = node.attr() else {
        return nil(ruby);
    };
    /* The case-PRESERVED local name is the suffix of the qualified name;
     * Lexbor's stored local_name is lower-cased even when the qualified name
     * keeps its case (set_attribute_ns is case-sensitive). */
    let (q, local) = (at.qualified_name(), at.local_name());
    if q.len() >= local.len() {
        dom_str(&q[q.len() - local.len()..])
    } else {
        dom_str(local)
    }
}

/// `#prefix` (DOM `prefix`): nil unless the qualified name is `prefix:local` -
/// typically nil for HTML5-parsed content. Element and Attribute only.
pub fn prefix(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    match qname(this.node()).and_then(|(q, local)| qname_prefix(q, local.len())) {
        Some(p) => dom_str(p),
        None => nil(ruby),
    }
}

/// `#namespace_uri` (DOM `namespaceURI`).
///
/// Element: resolved from `node.ns`, so - DOM-faithfully - an HTML element is in
/// the XHTML namespace rather than nil (an HTML element is never namespaceless;
/// this is what browsers' DOM and `namespace-uri()` return). SVG/MathML elements
/// get their own URI; nil only when truly unnamespaced.
///
/// Attribute: nil for an unprefixed attribute; for a prefixed one, the
/// parser-assigned foreign-content namespace keyed on the prefix.
///
/// Other kinds: nil.
pub fn namespace_uri(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let node = this.node();
    if node.element().is_some() {
        return ns_uri_str(ruby, node);
    }
    let Some(at) = node.attr() else {
        return nil(ruby);
    };

    /* An attribute set via set_attribute_ns records its OWN namespace on the
     * attr node - distinguishable because it differs from the owner element's
     * ns, which a parsed or normally-set attribute inherits. LXB_NS__UNDEF, set
     * by set_attribute_ns(nil, ...), is the null namespace and has no URI. */
    if at
        .owner()
        .is_some_and(|owner| owner.node().ns_id() != node.ns_id())
    {
        return ns_uri_str(ruby, node);
    }

    match qname(node).and_then(|(q, local)| qname_prefix(q, local.len())) {
        None => nil(ruby),
        Some(p) => match attr_ns_for_prefix(p) {
            Some(uri) => ruby.str_new(uri).as_value(),
            None => nil(ruby),
        },
    }
}

/// `Element#tag_name` (DOM `tagName`): the qualified name, uppercased for an
/// HTML element in an HTML document (`DIV`), as the DOM specifies - unlike
/// `#name`, which is the lowercase qualified name. SVG/MathML elements keep
/// their case. nil for a non-element.
pub fn tag_name(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    this.node()
        .element()
        .and_then(|el| el.tag_name())
        .map_or_else(|| nil(ruby), dom_str)
}

/// `ProcessingInstruction#target` (DOM `target`): the `xml` in `<?xml ...?>`.
/// nil for a non-PI. The PI's data is read with `#content` like any
/// character-data node.
pub fn pi_target(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    this.node().pi_target().map_or_else(|| nil(ruby), dom_str)
}

/// `#node_type`: the numeric DOM node type (`LXB_DOM_NODE_TYPE_*`).
pub fn node_type(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    ruby.integer_from_i64(this.node().node_type() as i64)
        .as_value()
}

/// `DocumentType#public_id` / `#system_id` (WHATWG DOM).
///
/// Lexbor represents a missing id inconsistently - NULL after `SYSTEM`, but an
/// empty string for a bare `<!DOCTYPE html>` - so empty is treated as absent and
/// both answer nil, matching Nokogiri. Defined only on DocumentType; for any
/// other receiver the handle answers None as well.
pub fn doctype_public_id(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    this.node()
        .doctype_public_id()
        .map_or_else(|| nil(ruby), dom_str)
}

pub fn doctype_system_id(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    this.node()
        .doctype_system_id()
        .map_or_else(|| nil(ruby), dom_str)
}

/// `Element#content_fragment`: a `<template>` element's "template contents" -
/// the separate DocumentFragment the HTML parser fills instead of making the
/// parsed nodes children of the `<template>` (WHATWG DOM
/// `HTMLTemplateElement.content`; browsers behave the same, with
/// `template.children` empty and `template.content` holding the nodes).
///
/// nil for any node that is not an HTML `<template>`. CSS and XPath over the
/// template ELEMENT deliberately do not descend into the content - matching the
/// DOM, and unavoidable for CSS, which runs Lexbor's selector engine over the
/// real tree - so query the fragment instead.
pub fn content_fragment(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    match this.node().template_content() {
        // SAFETY: the contents fragment belongs to the receiver's document.
        Some(content) => unsafe { wrap_node(Some(content), this.document) },
        None => nil(ruby),
    }
}

/// `#content` / `#text` / `#inner_text`: the concatenated text of this node and
/// its descendants.
///
/// The DOM makes a Document's textContent null; this returns the ROOT element's
/// text instead, which is the intuitive, Nokogiri-like `Document#text`.
pub fn content(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    let mut node = this.node();
    if node.node_type() == ty::DOCUMENT {
        match node.document_root() {
            Some(root) => node = root,
            None => return Ok(ruby.str_new("").as_value()),
        }
    }

    if matches!(node.node_type(), ty::ELEMENT | ty::FRAGMENT) {
        return element_text(ruby, this.document, node);
    }

    /* Character data and the other kinds keep the general path. A UTF-8
     * String, not magnus's str_from_slice: that one tags the String
     * ASCII-8BIT, and a binary Text#content poisons every UTF-8 String it is
     * appended to. */
    Ok(node.with_text_content(|text| match text {
        Some(bytes) => dom_str(bytes),
        None => ruby.str_new("").as_value(),
    }))
}

/// The element/fragment half of [`content`], which is the common case and
/// includes whole-document text.
///
/// Preferred: the per-document text index maps the node to the contiguous,
/// document-order run of its descendants' text slices, so this is one pre-sized
/// memcpy run with no per-extraction tree walk - the walk is otherwise the
/// dominant, cache-bound cost.
///
/// Fallback, when the index cannot serve this node (outside the indexed tree -
/// a fragment - or a build OOM): an iterative pre-order walk that appends each
/// text/CDATA node's data, stack-safe and skipping Lexbor's intermediate arena
/// buffer and copy.
fn element_text(ruby: &Ruby, document: Value, node: HtmlNode<'_>) -> Result<Value, Error> {
    // SAFETY: `document` is the node's live Document, and the slices the index
    // hands back are copied into the String before anything can change it.
    unsafe {
        let parsed = crate::glue::doc::doc_parsed_known(document);
        if let Some((slices, total)) = parsed.as_mut().and_then(|p| p.text_slices(node.as_raw())) {
            return Ok(Value::from_raw(ruby_str_from_slices(slices, total)?));
        }
    }

    let str = ruby.str_new("");
    let mut cur = node.first_child();
    while let Some(c) = cur {
        if let Some(data) = c.char_data() {
            if !data.is_empty() {
                /* `str` is a live local root, so growing it here is fine; the
                 * data is arena memory, not a Ruby buffer. */
                str.cat(data);
            }
        }
        cur = c.preorder_next(node);
    }
    Ok(str.as_value())
}

/* ------------------------------------------------------------------ *
 * tree navigation                                                    *
 * ------------------------------------------------------------------ */

pub fn get_document(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    this.document
}

/// `#parent`. An attribute has no `node.parent` - Lexbor never links one back to
/// its element - so it resolves through the compat attr->owner index.
///
/// The index is built explicitly, and a failed build raises, because an owner
/// lookup with no index would answer NULL for BOTH "this attribute is
/// not in the document" and "the index could not be allocated". Taking the
/// second as the first makes an owned attribute report no parent - a navigation
/// answer indistinguishable from the truthful one - so an allocation failure
/// raises here instead. (The OOM sweep found this: `Attr#parent` degraded from
/// `"svg"` to `nil` under injection, in the C original as much as here.)
pub fn parent(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    let node = this.node();
    let document = this.document;
    if node.attr().is_some() {
        // SAFETY: `document` is the attribute's live Document; the owner the
        // index answers belongs to it.
        unsafe {
            let index = doc_parsed(document)?.as_mut().and_then(|p| p.dom_index());
            let Some(index) = index else {
                return Err(Error::new(
                    error_class(),
                    "could not build the attribute index (out of memory)",
                ));
            };
            let owner = index.owner_of(node.as_raw() as *const LxbAttr);
            return Ok(wrap(owner, document));
        }
    }
    // SAFETY: the parent is in the receiver's tree.
    Ok(unsafe { wrap_node(node.parent(), document) })
}

pub fn next(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    // SAFETY: a sibling is in the receiver's tree.
    unsafe { wrap_node(this.node().next(), this.document) }
}

pub fn previous(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    // SAFETY: a sibling is in the receiver's tree.
    unsafe { wrap_node(this.node().prev(), this.document) }
}

/// The first node from `start` along `step` that is an element. `step` is a
/// generic rather than a `fn` pointer, so each walk inlines its link read.
#[inline]
fn first_element<'d>(
    start: Option<HtmlNode<'d>>,
    step: impl Fn(HtmlNode<'d>) -> Option<HtmlNode<'d>>,
) -> Option<HtmlNode<'d>> {
    let mut n = start;
    while let Some(x) = n {
        if x.element().is_some() {
            return Some(x);
        }
        n = step(x);
    }
    None
}

pub fn next_element(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let found = first_element(this.node().next(), HtmlNode::next);
    // SAFETY: a sibling is in the receiver's tree.
    unsafe { wrap_node(found, this.document) }
}

pub fn previous_element(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let found = first_element(this.node().prev(), HtmlNode::prev);
    // SAFETY: a sibling is in the receiver's tree.
    unsafe { wrap_node(found, this.document) }
}

/// `#child`: the first child node of any type, or nil.
pub fn child(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    // SAFETY: a child is in the receiver's tree.
    unsafe { wrap_node(this.node().first_child(), this.document) }
}

pub fn first_element_child(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let found = first_element(this.node().first_child(), HtmlNode::next);
    // SAFETY: a child is in the receiver's tree.
    unsafe { wrap_node(found, this.document) }
}

pub fn last_element_child(_ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let found = first_element(this.node().last_child(), HtmlNode::prev);
    // SAFETY: a child is in the receiver's tree.
    unsafe { wrap_node(found, this.document) }
}

/// Collect nodes into a NodeSet. The set is a live Ruby object across every
/// push, so nothing borrowed from Ruby is held here.
fn set_of<'d>(
    document: Value,
    nodes: impl Iterator<Item = HtmlNode<'d>>,
    elements_only: bool,
) -> Result<Value, Error> {
    // SAFETY: every node is in the tree whose keepalive Document is `document`.
    unsafe {
        let set = node_set_new(document);
        for n in nodes {
            if !elements_only || n.element().is_some() {
                node_set_push(set.as_raw(), n.as_raw() as *mut core::ffi::c_void)?;
            }
        }
        Ok(set)
    }
}

/// `#children`: every child node, as a NodeSet.
pub fn children(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    set_of(this.document, this.node().children(), false)
}

/// `#element_children` / `#elements`: the child elements only.
pub fn element_children(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    set_of(this.document, this.node().children(), true)
}

/// `#ancestors`: the ancestor elements, nearest first.
pub fn ancestors(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    set_of(this.document, this.node().ancestors(), true)
}

/* ------------------------------------------------------------------ *
 * attributes (read-only)                                             *
 * ------------------------------------------------------------------ */

/// `node[name]` -> the value String, or nil when absent or not an element.
///
/// This goes through Lexbor's attribute-name hash, which is keyed by LOCAL name
/// and lower-cases the lookup - see [`attribute_by_qualified_name`] for the
/// exact-match sibling and why both exist.
pub fn aref(ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let Some(el) = this.node().element() else {
        return Ok(nil(ruby));
    };
    // SAFETY: the guard keeps the name String reachable, and its bytes are only
    // read before the answer String is built.
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    let name = unsafe { nv.bytes() };
    if !el.has_attribute(name) {
        return Ok(nil(ruby));
    }
    let value = el.get_attribute(name).unwrap_or(&[]);
    Ok(dom_str(value))
}

/// `node.key?(name)`.
pub fn has_key(ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let Some(el) = this.node().element() else {
        return Ok(ruby.qfalse().as_value());
    };
    // SAFETY: the guard keeps the name String reachable while its bytes are read.
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    let has = el.has_attribute(unsafe { nv.bytes() });
    Ok(if has {
        ruby.qtrue().as_value()
    } else {
        ruby.qfalse().as_value()
    })
}

/// `node.keys` -> the attribute names, in document order.
pub fn keys(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let ary = ruby.ary_new();
    if let Some(el) = this.node().element() {
        for at in el.attrs() {
            let _ = ary.push(dom_str(at.qualified_name()));
        }
    }
    ary.as_value()
}

/// `node.values` -> the attribute values, in document order.
pub fn values(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    let ary = ruby.ary_new();
    if let Some(el) = this.node().element() {
        for at in el.attrs() {
            let _ = ary.push(dom_str(at.value()));
        }
    }
    ary.as_value()
}

/// `element.attribute_nodes` -> a NodeSet of Attribute nodes, in document order.
/// Empty for a non-element. These wrap the bare `lxb_dom_attr_t`; navigating
/// back with `Attribute#parent` goes through the compat attr->owner index.
pub fn attribute_nodes(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    let node = this.node();
    let attrs = node.element().into_iter().flat_map(|el| el.attrs());
    set_of(this.document, attrs.map(|at| at.node()), false)
}

/// `element.attribute_by_qualified_name(name)` -> the Attr whose QUALIFIED name
/// is exactly `name`, or nil.
///
/// `#[]` and `#key?` cannot answer this: they go through Lexbor's attribute-name
/// hash, which is keyed by LOCAL name, so on an element carrying a prefixed
/// attribute - `<a xlink:href>` in an inline `<svg>`, say - `el["href"]` hands
/// that attribute back. The DOM's by-name family (getAttribute, setAttribute,
/// removeAttribute) is defined on the qualified name, where `getAttribute("href")`
/// is null there, and needs the exact match.
///
/// The match is also BYTE-exact, where `#[]` lower-cases what it looks up.
/// getAttribute's ASCII-lowercasing applies only to an HTML element in an HTML
/// document, so the caller does that step.
pub fn attribute_by_qualified_name(
    ruby: &Ruby,
    this: super::HtmlSelf,
    rb_name: Value,
) -> Result<Value, Error> {
    let Some(el) = this.node().element() else {
        return Ok(nil(ruby));
    };
    // SAFETY: the guard keeps the name String reachable while its bytes are read.
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    // SAFETY: the guard keeps the String reachable; nothing allocates meanwhile.
    let name = unsafe { nv.bytes() };
    let found = el.attrs().find(|at| at.qualified_name() == name);
    /* The name is not read past here; wrapping allocates, so it happens after. */
    drop(nv);
    Ok(match found {
        // SAFETY: the attribute is in the receiver's tree.
        Some(at) => unsafe { wrap_node(Some(at.node()), this.document) },
        None => nil(ruby),
    })
}

/// `element.attribute_value_by_qualified_name(name)` -> that attribute's value
/// String, or nil.
///
/// The same match as [`attribute_by_qualified_name`] without wrapping an Attr:
/// this is the shape a DOM `getAttribute` / `hasAttribute` wants, and those run
/// often enough for the wrapper to show up. An empty value answers `""`, which
/// is how `hasAttribute` tells it from an absent attribute.
pub fn attribute_value_by_qualified_name(
    ruby: &Ruby,
    this: super::HtmlSelf,
    rb_name: Value,
) -> Result<Value, Error> {
    let Some(el) = this.node().element() else {
        return Ok(nil(ruby));
    };
    // SAFETY: the guard keeps the name String reachable while its bytes are read.
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    // SAFETY: the guard keeps the String reachable; nothing allocates meanwhile.
    let name = unsafe { nv.bytes() };
    let value = el
        .attrs()
        .find(|at| at.qualified_name() == name)
        .map(|at| at.value());
    drop(nv);
    Ok(value.map_or_else(|| nil(ruby), dom_str))
}

/// `attr.value`. For a non-attribute node this falls back to text content,
/// matching the loose Nokogiri-ish meaning of `#value`.
pub fn value(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    match this.node().attr() {
        Some(at) => Ok(dom_str(at.value())),
        None => content(ruby, this),
    }
}

/// `#line` -> the 1-based source line, or nil when unknown.
///
/// The line comes from the byte offset stamped onto the node at parse time,
/// resolved against the document's line table. nil for a node the tracker could
/// not place - a parser-inserted implicit `<html>`/`<head>`/`<body>`, a text or
/// comment node - never a wrong line.
pub fn line(ruby: &Ruby, this: super::HtmlSelf) -> Value {
    // SAFETY: `this.document` is the node's live Document.
    let n = unsafe {
        let p = crate::glue::doc::doc_parsed_known(this.document);
        p.as_ref().map_or(0, |p| p.node_line(this.node().as_raw()))
    };
    if n == 0 {
        nil(ruby)
    } else {
        ruby.integer_from_u64(n as u64).as_value()
    }
}

/* ------------------------------------------------------------------ *
 * document order                                                     *
 * ------------------------------------------------------------------ */

/// `#<=>`: document (pre-order) position, so an array of nodes can be sorted.
///
/// nil when the nodes are not comparable: a non-node, an XML node, different
/// documents or detached subtrees with no common root, or an attribute node -
/// attributes are not in the `first_child`/`next` chain, so their order is not
/// defined here. Included via Comparable, which supplies `<`, `>`, `between?`
/// and the rest.
pub fn spaceship(ruby: &Ruby, this: super::HtmlSelf, other: Value) -> Result<Value, Error> {
    let nil = nil(ruby);
    let int = |i: i64| ruby.integer_from_i64(i).as_value();

    /* A non-node, or an XML node - never order-comparable to an HTML one, and
     * asking is how we avoid arg_node's TypeError below. */
    let comparable = is_kind_of(other, &CLASS_NODE)
        && !is_kind_of(
            crate::glue::abi::keepalive_document(other)?,
            &CLASS_XML_DOCUMENT,
        );
    if !comparable {
        return Ok(nil);
    }

    let a = this.node();
    let b = arg_node(&other)?;
    if a == b {
        return Ok(int(0));
    }
    if a.attr().is_some() || b.attr().is_some() || !a.same_document(b) {
        return Ok(nil);
    }

    let (da, db) = (a.ancestors().count(), b.ancestors().count());
    let (mut pa, mut pb) = (a, b);

    /* Raise the deeper node to the other's depth; landing ON the other makes
     * that other an ancestor, which comes first in pre-order. */
    if da > db {
        for _ in 0..(da - db) {
            let Some(p) = pa.parent() else { return Ok(nil) };
            pa = p;
        }
        if pa == b {
            return Ok(int(1));
        }
    } else if db > da {
        for _ in 0..(db - da) {
            let Some(p) = pb.parent() else { return Ok(nil) };
            pb = p;
        }
        if pb == a {
            return Ok(int(-1));
        }
    }

    /* Climb both until they share a parent (the lowest common ancestor). A
     * missing parent on either side means different trees, or two roots. */
    let parent = loop {
        let (Some(qa), Some(qb)) = (pa.parent(), pb.parent()) else {
            return Ok(nil);
        };
        if qa == qb {
            break qa;
        }
        pa = qa;
        pb = qb;
    };

    /* pa and pb are distinct siblings: earlier in the child list is first. A
     * wide parent makes this the hot loop of a sort, so it is written out. */
    let mut c = parent.first_child();
    while let Some(x) = c {
        if x == pa {
            return Ok(int(-1));
        }
        if x == pb {
            return Ok(int(1));
        }
        c = x.next();
    }
    Ok(nil) /* unreachable for a well-formed tree */
}
