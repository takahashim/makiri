//! The HTML node's readers: name and namespace, the DTD identifiers, content,
//! navigation, attributes, source line and document order.
//!
//! Every read of the Lexbor tree goes through the typed handles of
//! `lexbor::adapter::html`, the node and text index through `bridge::wrapper` and `bridge::html`, and
//! the NodeSet through its fill handle, so the readers are all safe.
//!
//! # Where a GC may run
//!
//! Building any String or NodeSet is a GC point, so a view borrowed from a Ruby
//! String must not be live across one. The attribute lookups take their name
//! through `ruby_verified_text`, whose guard keeps the String reachable until
//! it drops, and read the bytes before building anything. Bytes borrowed from
//! the document are arena memory, which a GC does not move.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, Ruby, Value};

use super::{with_arg_node, wrap_node};
use crate::bridge::html::{dom_str, text_index_string, wrap_html_node};
use crate::bridge::node_set::node_set_with_fill;
use crate::bridge::ruby::is_kind_of;
use crate::bridge::string::{dom_name_str, ruby_verified_text};
use crate::child_index;
use crate::init::{CLASS_NODE, CLASS_XML_DOCUMENT};
use crate::lexbor::adapter::html::{HtmlAttr, HtmlElement, HtmlNode, HtmlTree, NodeType, RawNode};

/* ------------------------------------------------------------------ *
 * small helpers                                                      *
 * ------------------------------------------------------------------ */

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

/* ------------------------------------------------------------------ *
 * name / type / content                                              *
 * ------------------------------------------------------------------ */

/// `#name`. Matches Nokogiri: the lowercase tag name for an HTML element
/// (Lexbor lowercases during tokenization), and the un-prefixed DOM names
/// `text` / `comment` / `#cdata-section` / `document` for the other kinds.
pub fn name(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let node = this.node();
        if let Some((q, _)) = qname(node) {
            return Ok(dom_str(q));
        }
        Ok(match node.node_type() {
            NodeType::Text => ruby.str_new("text").as_value(),
            NodeType::Comment => ruby.str_new("comment").as_value(),
            NodeType::CDataSection => ruby.str_new("#cdata-section").as_value(),
            NodeType::Document => ruby.str_new("document").as_value(),
            _ => dom_str(node.node_name()),
        })
    })
}

/// `#local_name` (DOM `localName`): the name without any prefix - `div` for
/// `<div>`, `path` for an SVG `<path>`, `href` for an `xlink:href` attribute.
/// Element and Attribute only; the DOM gives a Text/Comment/Document none.
pub fn local_name(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        /* The DOM's case-preserved name - `foreignObject`, `refX` - where Lexbor
         * stores a lower-cased one; the same answer XPath's `local-name()` gives. */
        let node = this.node();
        let local = match (node.element(), node.attr()) {
            (Some(el), _) => el.dom_local_name(),
            (None, Some(at)) => at.dom_local_name(),
            (None, None) => return Ok(None),
        };
        Ok(Some(dom_name_str(local)))
    })
}

/// `#prefix` (DOM `prefix`): nil unless the qualified name is `prefix:local` -
/// typically nil for HTML5-parsed content. Element and Attribute only.
pub fn prefix(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(qname(this.node())
            .and_then(|(q, local)| qname_prefix(q, local.len()))
            .map(dom_name_str))
    })
}

/// `#namespace_uri` (DOM `namespaceURI`).
///
/// Element: resolved from `node.ns`, so - DOM-faithfully - an HTML element is in
/// the XHTML namespace rather than nil (an HTML element is never namespaceless;
/// this is what browsers' DOM and `namespace-uri()` return). SVG/MathML elements
/// get their own URI; nil only when truly unnamespaced.
///
/// Attribute: its OWN namespace (`HtmlAttr::own_ns_uri`) - the xlink one for a
/// parsed `xlink:href` in SVG, nil for an ordinary attribute - which is also
/// what XPath's `namespace-uri()` answers.
///
/// Other kinds: nil.
pub fn namespace_uri(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let node = this.node();
        let uri = match (node.element(), node.attr()) {
            (Some(_), _) => node.ns_uri(),
            (None, Some(at)) => at.own_ns_uri(),
            (None, None) => None,
        };
        Ok(uri.map(dom_name_str))
    })
}

/// `Element#tag_name` (DOM `tagName`): the qualified name, uppercased for an
/// HTML element in an HTML document (`DIV`), as the DOM specifies - unlike
/// `#name`, which is the lowercase qualified name. SVG/MathML elements keep
/// their case. nil for a non-element.
pub fn tag_name(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(this
            .node()
            .element()
            .and_then(|el| el.tag_name())
            .map(dom_name_str))
    })
}

/// `ProcessingInstruction#target` (DOM `target`): the `xml` in `<?xml ...?>`.
/// nil for a non-PI. The PI's data is read with `#content` like any
/// character-data node.
pub fn pi_target(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| this.with_node(|node| Ok(node.pi_target().map(dom_str))))
}

/// `#node_type`: the numeric DOM node type (`LXB_DOM_NODE_TYPE_*`).
pub fn node_type(_ruby: &Ruby, this: super::HtmlSelf) -> Result<i64, Error> {
    crate::bridge::ruby::entry(|| Ok(this.node().node_type() as u32 as i64))
}

/// `DocumentType#public_id` / `#system_id` (WHATWG DOM).
///
/// Lexbor represents a missing id inconsistently - NULL after `SYSTEM`, but an
/// empty string for a bare `<!DOCTYPE html>` - so empty is treated as absent and
/// both answer nil, matching Nokogiri. Defined only on DocumentType; for any
/// other receiver the handle answers None as well.
pub fn doctype_public_id(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| Ok(this.node().doctype_public_id().map(dom_str)))
}

pub fn doctype_system_id(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| Ok(this.node().doctype_system_id().map(dom_str)))
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
pub fn content_fragment(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| wrap_node(this.node().template_content(), this.document))
}

/// `#content` / `#text` / `#inner_text`: the concatenated text of this node and
/// its descendants.
///
/// The DOM makes a Document's textContent null; this returns the ROOT element's
/// text instead, which is the intuitive, Nokogiri-like `Document#text`.
pub fn content(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let mut node = this.node();
        if node.node_type() == NodeType::Document {
            match node.document_root() {
                Some(root) => node = root,
                None => return Ok(ruby.str_new("").as_value()),
            }
        }

        if matches!(
            node.node_type(),
            NodeType::Element | NodeType::DocumentFragment
        ) {
            return element_text(ruby, this.document, node);
        }

        /* Character data and attributes lend their own bytes; the other kinds
         * (a doctype) have none. A UTF-8
         * String, not magnus's str_from_slice: that one tags the String
         * ASCII-8BIT, and a binary Text#content poisons every UTF-8 String it is
         * appended to. */
        Ok(match node.own_text() {
            Some(bytes) => dom_str(bytes),
            None => ruby.str_new("").as_value(),
        })
    })
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
    if let Some(text) = text_index_string(document, node.into())? {
        return Ok(text);
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

pub fn get_document(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(this.document))
}

/// `#parent`. For an attribute, the element it is set on - Lexbor's own
/// `attr->owner`, read live (see `HtmlNode::parent`), so a removed attribute
/// answers nil and nothing needs building. (It used to go through an index
/// whose build could fail, which is why this returned `Result`.)
pub fn parent(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| wrap_node(this.node().parent(), this.document))
}

pub fn next(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| wrap_node(this.node().next(), this.document))
}

pub fn previous(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| wrap_node(this.node().prev(), this.document))
}

pub fn next_element(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let found = child_index::next_element_sibling(&HtmlTree::new(), this.node());
        wrap_node(found, this.document)
    })
}

pub fn previous_element(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let found = child_index::previous_element_sibling(&HtmlTree::new(), this.node());
        wrap_node(found, this.document)
    })
}

/// `#child`: the first child node of any type, or nil.
pub fn child(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| wrap_node(this.node().first_child(), this.document))
}

pub fn first_element_child(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let found = child_index::first_element_child(&HtmlTree::new(), this.node());
        wrap_node(found, this.document)
    })
}

pub fn last_element_child(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let found = child_index::last_element_child(&HtmlTree::new(), this.node());
        wrap_node(found, this.document)
    })
}

/// `#last_child`: the last child node of any type, or nil.
pub fn last_child(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| wrap_node(this.node().last_child(), this.document))
}

/// `#root_node` (DOM `getRootNode()`, not shadow-including) - see
/// [`crate::dom_rules::root`]. An Attr is its own root, though `#parent`
/// answers its element.
pub fn root_node(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let root = crate::dom_rules::root(&HtmlTree::new(), this.node());
        wrap_html_node(RawNode::from(root), this.document)
    })
}

/// Collect nodes into a NodeSet. The set is a live Ruby object across every
/// push, so nothing borrowed from Ruby is held here.
fn set_of<'d>(
    document: Value,
    nodes: impl Iterator<Item = HtmlNode<'d>>,
    elements_only: bool,
) -> Result<Value, Error> {
    let (set, fill) = node_set_with_fill(document);
    for n in nodes {
        if !elements_only || n.element().is_some() {
            fill.push(RawNode::from(n).into())?;
        }
    }
    Ok(set)
}

/// `#children`: every child node, as a NodeSet.
pub fn children(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| set_of(this.document, this.node().children(), false))
}

/// `#element_children` / `#elements`: the child elements only.
pub fn element_children(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| set_of(this.document, this.node().children(), true))
}

/// `#ancestors`: the ancestor elements, nearest first.
pub fn ancestors(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| set_of(this.document, this.node().ancestors(), true))
}

/* ------------------------------------------------------------------ *
 * attributes (read-only)                                             *
 * ------------------------------------------------------------------ */

/// `node[name]` -> the value String, or nil when absent or not an element.
///
/// DOM `getAttribute`: matched on the QUALIFIED name, ASCII-lowercased first on
/// an HTML element (`HtmlElement::attr_by_name`), so `svg_a["href"]` does not
/// find `xlink:href`. See [`attribute_by_qualified_name`] for the byte-exact
/// sibling.
pub fn aref(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(el) = this.node().element() else {
            return Ok(None);
        };
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        /* A present attribute with no value (`<input disabled>`) answers
         * `Some(b"")`, which is `""`; only an absent one is nil. */
        Ok(el.get_attribute(nv.as_bytes()).map(dom_str))
    })
}

/// `node.key?(name)`.
pub fn has_key(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<bool, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(el) = this.node().element() else {
            return Ok(false);
        };
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        Ok(el.has_attribute(nv.as_bytes()))
    })
}

/// `node.keys` -> the attribute names, in document order.
pub fn keys(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let ary = ruby.ary_new();
        if let Some(el) = this.node().element() {
            for at in el.attrs() {
                ary.push(dom_str(at.qualified_name()))?;
            }
        }
        Ok(ary.as_value())
    })
}

/// `node.values` -> the attribute values, in document order.
pub fn values(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let ary = ruby.ary_new();
        if let Some(el) = this.node().element() {
            for at in el.attrs() {
                ary.push(dom_str(at.value()))?;
            }
        }
        Ok(ary.as_value())
    })
}

/// `element.attribute_nodes` -> a NodeSet of Attribute nodes, in document order.
/// Empty for a non-element. These wrap the bare `lxb_dom_attr_t`; navigating
/// back with `Attribute#parent` reads its `attr->owner`.
pub fn attribute_nodes(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let node = this.node();
        let attrs = node.element().into_iter().flat_map(|el| el.attrs());
        set_of(this.document, attrs.map(|at| at.node()), false)
    })
}

/// `element.attribute_by_qualified_name(name)` -> the Attr whose QUALIFIED name
/// is exactly `name`, or nil.
///
/// `#[]` and `#key?` match the qualified name too, but lower-case what they
/// look up on an HTML element, as DOM getAttribute does; this match is
/// BYTE-exact, for a caller that does that step itself.
pub fn attribute_by_qualified_name(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_name: Value,
) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(el) = this.node().element() else {
            return Ok(None);
        };
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        let name = nv.as_bytes();
        let found = el.attrs().find(|at| at.qualified_name() == name);
        /* The name is not read past here; wrapping allocates, so it happens after. */
        drop(nv);
        wrap_node(found.map(|at| at.node()), this.document)
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
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_name: Value,
) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(el) = this.node().element() else {
            return Ok(None);
        };
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        let name = nv.as_bytes();
        let value = el
            .attrs()
            .find(|at| at.qualified_name() == name)
            .map(|at| at.value());
        drop(nv);
        Ok(value.map(dom_str))
    })
}

/// The attribute of `el` in namespace `rb_ns` (nil or "" for none) with local
/// name `rb_local` - DOM "get an attribute by namespace and local name", the
/// key `remove_attribute_ns` removes by. The namespace compares exactly.
fn attr_by_ns_arg<'d>(
    el: HtmlElement<'d>,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Option<HtmlAttr<'d>>, Error> {
    let lv = ruby_verified_text(rb_local, "attribute local name")?;
    let nv = crate::bridge::string::namespace_arg(rb_ns, "namespace")?;
    /* Looked up, not interned: a namespace the document never interned is
     * one no attribute here carries. */
    let want_ns = match nv.as_ref() {
        Some(uri) => match el.node().owner_document().lookup_ns(uri.as_bytes()) {
            Some(id) => Some(id),
            None => return Ok(None),
        },
        None => None,
    };
    Ok(el.attr_by_ns(want_ns, lv.as_bytes()))
}

/// `#attribute_value_ns(ns, local)` (DOM `getAttributeNS`): the value of the
/// attribute in namespace `ns` (nil or "" for none) with local name `local`,
/// or nil - for a non-element too.
pub fn attribute_value_ns(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(el) = this.node().element() else {
            return Ok(None);
        };
        let found = attr_by_ns_arg(el, rb_ns, rb_local)?;
        Ok(found.map(|at| dom_str(at.value())))
    })
}

/// `#attribute_node_ns(ns, local)` (DOM `getAttributeNodeNS`): the Attr
/// [`attribute_value_ns`] reads, or nil.
pub fn attribute_node_ns(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Option<Value>, Error> {
    crate::bridge::ruby::entry(|| {
        let Some(el) = this.node().element() else {
            return Ok(None);
        };
        let found = attr_by_ns_arg(el, rb_ns, rb_local)?;
        wrap_node(found.map(|at| at.node()), this.document)
    })
}

/// `attr.value`. For a non-attribute node this falls back to text content,
/// matching the loose Nokogiri-ish meaning of `#value`.
pub fn value(ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| match this.node().attr() {
        Some(at) => Ok(dom_str(at.value())),
        None => content(ruby, this),
    })
}

/// `#line` -> the 1-based source line, or nil when unknown.
///
/// The line comes from the byte offset stamped onto the node at parse time,
/// resolved against the document's line table. nil for a node the tracker could
/// not place - a parser-inserted implicit `<html>`/`<head>`/`<body>`, a text or
/// comment node - never a wrong line.
pub fn line(this: super::HtmlSelf) -> Result<Option<usize>, Error> {
    crate::bridge::ruby::entry(|| Ok(crate::bridge::html::node_line(this.document, this.raw())))
}

/* ------------------------------------------------------------------ *
 * document order                                                     *
 * ------------------------------------------------------------------ */

/// `#<=>`: document (pre-order) position, so an array of nodes can be sorted.
///
/// nil when the nodes are not comparable: a non-node, an XML node, or any pair
/// `HtmlNode::document_order` does not order (different documents, detached
/// subtrees with no common root, an attribute node). Included via Comparable,
/// which supplies `<`, `>`, `between?` and the rest.
pub fn spaceship(_ruby: &Ruby, this: super::HtmlSelf, other: Value) -> Result<Option<i64>, Error> {
    crate::bridge::ruby::entry(|| {
        /* A non-node, or an XML node - never order-comparable to an HTML one, and
         * asking is how we avoid with_arg_node's TypeError below. */
        let comparable = is_kind_of(other, &CLASS_NODE)
            && !is_kind_of(
                crate::bridge::wrapper::keepalive_document(other)?,
                &CLASS_XML_DOCUMENT,
            );
        if !comparable {
            return Ok(None);
        }
        let order = with_arg_node(other, |o| this.node().document_order(o))?;
        Ok(order.map(|o| o as i64))
    })
}
