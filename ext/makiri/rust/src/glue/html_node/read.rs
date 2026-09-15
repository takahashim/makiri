//! The HTML node's readers: name and namespace, the DTD identifiers, content,
//! navigation, attributes, source line and document order.
//!
//! Every Lexbor accessor is imported from `glue::abi`, which has the crate's one
//! declaration of each.
//!
//! # Where a GC may run
//!
//! Building any String or NodeSet is a GC point, so a view borrowed from a Ruby
//! String must not be live across one. The attribute lookups take their name
//! through `mkr_ruby_verified_text` (which anchors the String in the returned
//! view) and are written so the anchor outlives the last use of its bytes; the
//! `let _anchor` at the end of each is that lifetime, spelled out, and is the
//! Rust equivalent of the C's `RB_GC_GUARD`.

use core::ffi::c_char;

use magnus::rb_sys::FromRawValue;
use magnus::{prelude::*, Error, RArray, Ruby, Value};

use super::ty;
use super::{node_document, unwrap, wrap};
use crate::glue::abi::{
    error_class, is_kind_of, lxb_dom_attr_local_name, lxb_dom_attr_qualified_name,
    lxb_dom_attr_value_noi, lxb_dom_document_destroy_text_noi, lxb_dom_document_root,
    lxb_dom_document_type_public_id_noi, lxb_dom_document_type_system_id_noi,
    lxb_dom_element_first_attribute_noi, lxb_dom_element_get_attribute,
    lxb_dom_element_has_attribute, lxb_dom_element_local_name, lxb_dom_element_next_attribute_noi,
    lxb_dom_element_qualified_name, lxb_dom_element_tag_name, lxb_dom_node_name,
    lxb_dom_node_text_content, lxb_ns_by_id, mkr_cNode, mkr_cXmlDocument, mkr_doc_parsed,
    mkr_node_set_new, mkr_node_set_push, mkr_ruby_str_from_borrowed, mkr_ruby_str_from_slices,
    mkr_ruby_verified_text, LxbAttr, LxbDoc, LxbElement, LxbNode,
};
use crate::lexbor_abi as lxb;
use crate::xpath_abi::VerifiedText as BorrowedText;

const NS_UNDEF: usize = lxb::lxb_ns_id_enum_t_LXB_NS__UNDEF as usize;
const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;
const TAG_TEMPLATE: usize = lxb::lxb_tag_id_enum_t_LXB_TAG_TEMPLATE as usize;

pub use crate::dom_adapter::dom_index::mkr_parsed_attr_owner;
pub use crate::dom_adapter::dom_index::mkr_parsed_dom_index_build;
pub use crate::dom_adapter::source_loc::mkr_parsed_node_line;
pub use crate::dom_adapter::text_index::mkr_parsed_text_slices;

/* ------------------------------------------------------------------ *
 * small helpers                                                      *
 * ------------------------------------------------------------------ */

#[inline]
fn borrowed(p: *const u8, len: usize) -> BorrowedText {
    unsafe { BorrowedText::from_raw_parts(p as *const c_char, len) }
}

/// A UTF-8 String over Lexbor's interned bytes. They live in the document arena
/// and outlive the call, so there is nothing to anchor.
#[inline]
unsafe fn str_of(p: *const u8, len: usize) -> Value {
    Value::from_raw(mkr_ruby_str_from_borrowed(borrowed(p, len)))
}

/// An Element's or Attribute's qualified name with the length of its local part,
/// or `None` for any other kind. The one place the element-vs-attribute accessor
/// pair is chosen.
unsafe fn qname(node: *mut LxbNode) -> Option<(*const u8, usize, usize)> {
    let (mut qlen, mut llen) = (0usize, 0usize);
    match (*node).type_ {
        ty::ELEMENT => {
            let el = node as *mut LxbElement;
            let q = lxb_dom_element_qualified_name(el, &mut qlen);
            lxb_dom_element_local_name(el, &mut llen);
            Some((q, qlen, llen))
        }
        ty::ATTRIBUTE => {
            let at = node as *mut LxbAttr;
            let q = lxb_dom_attr_qualified_name(at, &mut qlen);
            lxb_dom_attr_local_name(at, &mut llen);
            Some((q, qlen, llen))
        }
        _ => None,
    }
}

/// The prefix slice of a qualified name whose local part is `llen` bytes
/// (qualified is `prefix:local` or a bare `local`).
///
/// Centralises the `qlen`-vs-`llen + 1` boundary so [`prefix`] and
/// [`namespace_uri`] cannot drift apart on it, and so a colon inside a local
/// name is never mistaken for the separator.
#[inline]
unsafe fn qname_prefix<'a>(q: *const u8, qlen: usize, llen: usize) -> Option<&'a [u8]> {
    if q.is_null() || qlen <= llen + 1 {
        return None;
    }
    Some(core::slice::from_raw_parts(q, qlen - llen - 1))
}

/// An interned namespace id as its URI String, or nil. The one place an lxb
/// ns-id becomes a Ruby URI.
unsafe fn ns_uri_of_id(ruby: &Ruby, node: *mut LxbNode) -> Value {
    if (*node).ns == NS_UNDEF {
        return ruby.qnil().as_value();
    }
    let doc = (*node).owner_document;
    if doc.is_null() || (*doc).ns.is_null() {
        return ruby.qnil().as_value();
    }
    let mut len = 0usize;
    let uri = lxb_ns_by_id((*doc).ns, (*node).ns, &mut len);
    if uri.is_null() || len == 0 {
        return ruby.qnil().as_value();
    }
    str_of(uri, len)
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
pub fn name(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        let mut len = 0usize;
        match (*node).type_ {
            ty::ELEMENT => {
                let p = lxb_dom_element_qualified_name(node as *mut LxbElement, &mut len);
                str_of(p, len)
            }
            ty::ATTRIBUTE => {
                let p = lxb_dom_attr_qualified_name(node as *mut LxbAttr, &mut len);
                str_of(p, len)
            }
            ty::TEXT => ruby.str_new("text").as_value(),
            ty::COMMENT => ruby.str_new("comment").as_value(),
            ty::CDATA => ruby.str_new("#cdata-section").as_value(),
            ty::DOCUMENT => ruby.str_new("document").as_value(),
            _ => str_of(lxb_dom_node_name(node, &mut len), len),
        }
    }
}

/// `#local_name` (DOM `localName`): the name without any prefix - `div` for
/// `<div>`, `path` for an SVG `<path>`, `href` for an `xlink:href` attribute.
/// Element and Attribute only; the DOM gives a Text/Comment/Document none.
pub fn local_name(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        let mut len = 0usize;
        match (*node).type_ {
            ty::ELEMENT => {
                let p = lxb_dom_element_local_name(node as *mut LxbElement, &mut len);
                str_of(p, len)
            }
            ty::ATTRIBUTE => {
                /* The case-PRESERVED local name is the suffix of the qualified
                 * name; Lexbor's stored local_name is lower-cased even when the
                 * qualified name keeps its case (set_attribute_ns is
                 * case-sensitive). */
                let at = node as *mut LxbAttr;
                let mut qlen = 0usize;
                let mut llen = 0usize;
                let q = lxb_dom_attr_qualified_name(at, &mut qlen);
                lxb_dom_attr_local_name(at, &mut llen);
                if !q.is_null() && qlen >= llen {
                    str_of(q.add(qlen - llen), llen)
                } else {
                    let p = lxb_dom_attr_local_name(at, &mut len);
                    str_of(p, len)
                }
            }
            _ => ruby.qnil().as_value(),
        }
    }
}

/// `#prefix` (DOM `prefix`): nil unless the qualified name is `prefix:local` -
/// typically nil for HTML5-parsed content. Element and Attribute only.
pub fn prefix(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        match qname(node).and_then(|(q, ql, ll)| qname_prefix(q, ql, ll)) {
            Some(p) => str_of(p.as_ptr(), p.len()),
            None => ruby.qnil().as_value(),
        }
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
pub fn namespace_uri(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);

        if (*node).type_ == ty::ELEMENT {
            return ns_uri_of_id(ruby, node);
        }
        if (*node).type_ != ty::ATTRIBUTE {
            return ruby.qnil().as_value();
        }

        /* An attribute set via set_attribute_ns records its OWN namespace on
         * the attr node - distinguishable because it differs from the owner
         * element's ns, which a parsed or normally-set attribute inherits.
         * LXB_NS__UNDEF, set by set_attribute_ns(nil, ...), is the null
         * namespace and ns_uri_of_id answers nil for it. */
        let at = node as *mut LxbAttr;
        let owner = (*at).owner;
        if !owner.is_null() && (*node).ns != (*owner).node.ns {
            return ns_uri_of_id(ruby, node);
        }

        match qname(node).and_then(|(q, ql, ll)| qname_prefix(q, ql, ll)) {
            None => ruby.qnil().as_value(),
            Some(p) => match attr_ns_for_prefix(p) {
                Some(uri) => ruby.str_new(uri).as_value(),
                None => ruby.qnil().as_value(),
            },
        }
    }
}

/// `Element#tag_name` (DOM `tagName`): the qualified name, uppercased for an
/// HTML element in an HTML document (`DIV`), as the DOM specifies - unlike
/// `#name`, which is the lowercase qualified name. SVG/MathML elements keep
/// their case. nil for a non-element.
pub fn tag_name(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ELEMENT {
            return ruby.qnil().as_value();
        }
        let mut len = 0usize;
        let p = lxb_dom_element_tag_name(node as *mut LxbElement, &mut len);
        if p.is_null() {
            return ruby.qnil().as_value();
        }
        str_of(p, len)
    }
}

/// `ProcessingInstruction#target` (DOM `target`): the `xml` in `<?xml ...?>`.
/// nil for a non-PI. The PI's data is read with `#content` like any
/// character-data node.
pub fn pi_target(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        if (*node).type_ != ty::PI {
            return ruby.qnil().as_value();
        }
        let mut len = 0usize;
        let p = lxb_dom_processing_instruction_target_noi(node as *mut _, &mut len);
        str_of(p, len)
    }
}

use crate::glue::abi::lxb_dom_processing_instruction_target_noi;

/// `#node_type`: the numeric DOM node type (`LXB_DOM_NODE_TYPE_*`).
pub fn node_type(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        ruby.integer_from_i64((*unwrap(rb_self)).type_ as i64)
            .as_value()
    }
}

/// `DocumentType#public_id` / `#system_id` (WHATWG DOM).
///
/// Lexbor represents a missing id inconsistently - NULL after `SYSTEM`, but an
/// empty string for a bare `<!DOCTYPE html>` - so empty is treated as absent and
/// both answer nil, matching Nokogiri. Defined only on DocumentType, so the
/// receiver is always a doctype; the guard is belt-and-braces.
unsafe fn doctype_id(ruby: &Ruby, rb_self: Value, system: bool) -> Value {
    let node = unwrap(rb_self);
    if (*node).type_ != ty::DOCTYPE {
        return ruby.qnil().as_value();
    }
    let mut len = 0usize;
    let dt = node as *mut _;
    let id = if system {
        lxb_dom_document_type_system_id_noi(dt, &mut len)
    } else {
        lxb_dom_document_type_public_id_noi(dt, &mut len)
    };
    if id.is_null() || len == 0 {
        return ruby.qnil().as_value();
    }
    str_of(id, len)
}

pub fn doctype_public_id(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { doctype_id(ruby, rb_self, false) }
}

pub fn doctype_system_id(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { doctype_id(ruby, rb_self, true) }
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
pub fn content_fragment(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ELEMENT
            || (*node).local_name != TAG_TEMPLATE
            || (*node).ns != NS_HTML
        {
            return ruby.qnil().as_value();
        }
        let content = (*(node as *mut lxb::lxb_html_template_element_t)).content;
        if content.is_null() {
            return ruby.qnil().as_value();
        }
        wrap(content as *mut LxbNode, node_document(rb_self))
    }
}

/// `#content` / `#text` / `#inner_text`: the concatenated text of this node and
/// its descendants.
///
/// The DOM makes a Document's textContent null; this returns the ROOT element's
/// text instead, which is the intuitive, Nokogiri-like `Document#text`.
pub fn content(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let mut node = unwrap(rb_self);
        if (*node).type_ == ty::DOCUMENT {
            node = lxb_dom_document_root(node as *mut LxbDoc);
            if node.is_null() {
                return ruby.str_new("").as_value();
            }
        }

        if (*node).type_ == ty::ELEMENT || (*node).type_ == ty::FRAGMENT {
            return element_text(ruby, rb_self, node);
        }

        /* Character data and the other kinds keep the general path. */
        let mut len = 0usize;
        let text = lxb_dom_node_text_content(node, &mut len);
        if text.is_null() {
            return ruby.str_new("").as_value();
        }
        /* rb_utf8_str_new, not magnus's str_from_slice: that one tags the
         * String ASCII-8BIT, and a binary Text#content poisons every UTF-8
         * String it is appended to. The DOM is always valid UTF-8 by the
         * text-input contract, so the tag is the whole difference. */
        let s = Value::from_raw(rb_sys::rb_utf8_str_new(
            text as *const c_char,
            len as core::ffi::c_long,
        ));
        lxb_dom_document_destroy_text_noi((*node).owner_document, text);
        s
    }
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
unsafe fn element_text(ruby: &Ruby, rb_self: Value, node: *mut LxbNode) -> Value {
    use magnus::rb_sys::AsRawValue;

    let parsed = mkr_doc_parsed(node_document(rb_self).as_raw());
    if !parsed.is_null() {
        let mut slices: *const BorrowedText = core::ptr::null();
        let mut n = 0usize;
        let mut total = 0usize;
        if mkr_parsed_text_slices(parsed, node, &mut slices, &mut n, &mut total) != 0 {
            return Value::from_raw(mkr_ruby_str_from_slices(slices, n, total));
        }
    }

    let str = ruby.str_new("");
    let mut cur = (*node).first_child;
    while !cur.is_null() {
        if (*cur).type_ == ty::TEXT || (*cur).type_ == ty::CDATA {
            let d = &(*(cur as *mut lxb::lxb_dom_character_data_t)).data;
            if !d.data.is_null() && d.length != 0 {
                /* `str` is a live local root, so growing it across this call is
                 * fine; `d.data` is arena memory, not a Ruby buffer. */
                rb_sys::rb_str_cat(str.as_raw(), d.data as *const c_char, d.length as _);
            }
        }
        if !(*cur).first_child.is_null() {
            cur = (*cur).first_child;
            continue;
        }
        while cur != node && (*cur).next.is_null() {
            cur = (*cur).parent;
        }
        if cur == node {
            break;
        }
        cur = (*cur).next;
    }
    str.as_value()
}

/* ------------------------------------------------------------------ *
 * tree navigation                                                    *
 * ------------------------------------------------------------------ */

pub fn get_document(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { node_document(rb_self) }
}

/// `#parent`. An attribute has no `node.parent` - Lexbor never links one back to
/// its element - so it resolves through the compat attr->owner index.
///
/// The index is built explicitly rather than left to `mkr_parsed_attr_owner`'s
/// lazy build, because that function answers NULL for BOTH "this attribute is
/// not in the document" and "the index could not be allocated". Taking the
/// second as the first makes an owned attribute report no parent - a navigation
/// answer indistinguishable from the truthful one - so an allocation failure
/// raises here instead. (The OOM sweep found this: `Attr#parent` degraded from
/// `"svg"` to `nil` under injection, in the C original as much as here.)
pub fn parent(ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let node = unwrap(rb_self);
        let document = node_document(rb_self);
        if (*node).type_ == ty::ATTRIBUTE {
            let parsed = mkr_doc_parsed(document.as_raw());
            if parsed.is_null() || !mkr_parsed_dom_index_build(parsed) {
                return Err(Error::new(
                    error_class(),
                    "could not build the attribute index (out of memory)",
                ));
            }
            let owner = mkr_parsed_attr_owner(parsed, node as *mut LxbAttr);
            return Ok(wrap(owner, document));
        }
        let _ = ruby;
        Ok(wrap((*node).parent, document))
    }
}

pub fn next(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { wrap((*unwrap(rb_self)).next, node_document(rb_self)) }
}

pub fn previous(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { wrap((*unwrap(rb_self)).prev, node_document(rb_self)) }
}

pub fn next_element(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let mut n = (*unwrap(rb_self)).next;
        while !n.is_null() && (*n).type_ != ty::ELEMENT {
            n = (*n).next;
        }
        wrap(n, node_document(rb_self))
    }
}

pub fn previous_element(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let mut n = (*unwrap(rb_self)).prev;
        while !n.is_null() && (*n).type_ != ty::ELEMENT {
            n = (*n).prev;
        }
        wrap(n, node_document(rb_self))
    }
}

/// `#child`: the first child node of any type, or nil.
pub fn child(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { wrap((*unwrap(rb_self)).first_child, node_document(rb_self)) }
}

pub fn first_element_child(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let mut c = (*unwrap(rb_self)).first_child;
        while !c.is_null() && (*c).type_ != ty::ELEMENT {
            c = (*c).next;
        }
        wrap(c, node_document(rb_self))
    }
}

pub fn last_element_child(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let mut c = (*unwrap(rb_self)).last_child;
        while !c.is_null() && (*c).type_ != ty::ELEMENT {
            c = (*c).prev;
        }
        wrap(c, node_document(rb_self))
    }
}

/// Collect a node chain into a NodeSet. The set is a live Ruby object across
/// every push, so nothing borrowed is held here.
unsafe fn set_of(
    rb_self: Value,
    start: *mut LxbNode,
    step: unsafe fn(*mut LxbNode) -> *mut LxbNode,
    elements_only: bool,
) -> Value {
    use magnus::rb_sys::AsRawValue;
    let document = node_document(rb_self);
    let set = mkr_node_set_new(document.as_raw());
    let mut n = start;
    while !n.is_null() {
        if !elements_only || (*n).type_ == ty::ELEMENT {
            mkr_node_set_push(set, n as *mut core::ffi::c_void);
        }
        n = step(n);
    }
    Value::from_raw(set)
}

unsafe fn step_next(n: *mut LxbNode) -> *mut LxbNode {
    (*n).next
}

unsafe fn step_parent(n: *mut LxbNode) -> *mut LxbNode {
    (*n).parent
}

/// `#children`: every child node, as a NodeSet.
pub fn children(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { set_of(rb_self, (*unwrap(rb_self)).first_child, step_next, false) }
}

/// `#element_children` / `#elements`: the child elements only.
pub fn element_children(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { set_of(rb_self, (*unwrap(rb_self)).first_child, step_next, true) }
}

/// `#ancestors`: the ancestor elements, nearest first.
pub fn ancestors(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe { set_of(rb_self, (*unwrap(rb_self)).parent, step_parent, true) }
}

/* ------------------------------------------------------------------ *
 * attributes (read-only)                                             *
 * ------------------------------------------------------------------ */

/// `node[name]` -> the value String, or nil when absent or not an element.
///
/// This goes through Lexbor's attribute-name hash, which is keyed by LOCAL name
/// and lower-cases the lookup - see [`attribute_by_qualified_name`] for the
/// exact-match sibling and why both exist.
pub fn aref(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ELEMENT {
            return ruby.qnil().as_value();
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let el = node as *mut LxbElement;
        let out = if !lxb_dom_element_has_attribute(el, nv.ptr as *const u8, nv.len) {
            ruby.qnil().as_value()
        } else {
            let mut vlen = 0usize;
            let val = lxb_dom_element_get_attribute(el, nv.ptr as *const u8, nv.len, &mut vlen);
            str_of(val, vlen)
        };
        let _anchor = nv.value;
        out
    }
}

/// `node.key?(name)`.
pub fn has_key(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ELEMENT {
            return ruby.qfalse().as_value();
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let has =
            lxb_dom_element_has_attribute(node as *mut LxbElement, nv.ptr as *const u8, nv.len);
        let _anchor = nv.value;
        if has {
            ruby.qtrue().as_value()
        } else {
            ruby.qfalse().as_value()
        }
    }
}

/// Walk an element's own attribute list. Empty for a non-element.
unsafe fn each_attr(node: *mut LxbNode, mut f: impl FnMut(*mut LxbAttr) -> bool) {
    if (*node).type_ != ty::ELEMENT {
        return;
    }
    let mut at = lxb_dom_element_first_attribute_noi(node as *mut LxbElement);
    while !at.is_null() {
        if !f(at) {
            return;
        }
        at = lxb_dom_element_next_attribute_noi(at);
    }
}

/// `node.keys` -> the attribute names, in document order.
pub fn keys(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let ary = ruby.ary_new();
        each_attr(unwrap(rb_self), |at| {
            let mut len = 0usize;
            let p = lxb_dom_attr_qualified_name(at, &mut len);
            let _ = ary.push(str_of(p, len));
            true
        });
        ary.as_value()
    }
}

/// `node.values` -> the attribute values, in document order.
pub fn values(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let ary: RArray = ruby.ary_new();
        each_attr(unwrap(rb_self), |at| {
            let mut len = 0usize;
            let p = lxb_dom_attr_value_noi(at, &mut len);
            let _ = ary.push(str_of(p, len));
            true
        });
        ary.as_value()
    }
}

/// `element.attribute_nodes` -> a NodeSet of Attribute nodes, in document order.
/// Empty for a non-element. These wrap the bare `lxb_dom_attr_t`; navigating
/// back with `Attribute#parent` goes through the compat attr->owner index.
pub fn attribute_nodes(_ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let set = mkr_node_set_new(node_document(rb_self).as_raw());
        each_attr(unwrap(rb_self), |at| {
            mkr_node_set_push(set, at as *mut core::ffi::c_void);
            true
        });
        Value::from_raw(set)
    }
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
pub fn attribute_by_qualified_name(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ELEMENT {
            return ruby.qnil().as_value();
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let want = nv.bytes();

        let mut found: *mut LxbAttr = core::ptr::null_mut();
        each_attr(node, |at| {
            let mut len = 0usize;
            let q = lxb_dom_attr_qualified_name(at, &mut len);
            if !q.is_null() && core::slice::from_raw_parts(q, len) == want {
                found = at;
                return false;
            }
            true
        });
        /* The anchor must outlive `want`, which the scan above reads; wrapping
         * allocates, so the wrap happens after. */
        let _anchor = nv.value;

        if found.is_null() {
            return ruby.qnil().as_value();
        }
        wrap(found as *mut LxbNode, node_document(rb_self))
    }
}

/// `element.attribute_value_by_qualified_name(name)` -> that attribute's value
/// String, or nil.
///
/// The same match as [`attribute_by_qualified_name`] without wrapping an Attr:
/// this is the shape a DOM `getAttribute` / `hasAttribute` wants, and those run
/// often enough for the wrapper to show up. An empty value answers `""`, which
/// is how `hasAttribute` tells it from an absent attribute.
pub fn attribute_value_by_qualified_name(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ELEMENT {
            return ruby.qnil().as_value();
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let want = nv.bytes();

        let mut val: *const u8 = core::ptr::null();
        let mut vlen = 0usize;
        let mut hit = false;
        each_attr(node, |at| {
            let mut len = 0usize;
            let q = lxb_dom_attr_qualified_name(at, &mut len);
            if !q.is_null() && core::slice::from_raw_parts(q, len) == want {
                val = lxb_dom_attr_value_noi(at, &mut vlen);
                hit = true;
                return false;
            }
            true
        });
        let _anchor = nv.value;

        if !hit {
            return ruby.qnil().as_value();
        }
        str_of(val, vlen)
    }
}

/// `attr.value`. For a non-attribute node this falls back to text content,
/// matching the loose Nokogiri-ish meaning of `#value`.
pub fn value(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        if (*node).type_ != ty::ATTRIBUTE {
            return content(ruby, rb_self);
        }
        let mut len = 0usize;
        let p = lxb_dom_attr_value_noi(node as *mut LxbAttr, &mut len);
        str_of(p, len)
    }
}

/// `#line` -> the 1-based source line, or nil when unknown.
///
/// The line comes from the byte offset stamped onto the node at parse time,
/// resolved against the document's line table. nil for a node the tracker could
/// not place - a parser-inserted implicit `<html>`/`<head>`/`<body>`, a text or
/// comment node - never a wrong line.
pub fn line(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let node = unwrap(rb_self);
        let p = mkr_doc_parsed(node_document(rb_self).as_raw());
        let n = mkr_parsed_node_line(p, node);
        if n == 0 {
            ruby.qnil().as_value()
        } else {
            ruby.integer_from_u64(n as u64).as_value()
        }
    }
}

/* ------------------------------------------------------------------ *
 * document order                                                     *
 * ------------------------------------------------------------------ */

/// Distance from `n` to the root (a node with no parent).
unsafe fn depth(n: *mut LxbNode) -> usize {
    let mut d = 0usize;
    let mut p = (*n).parent;
    while !p.is_null() {
        d += 1;
        p = (*p).parent;
    }
    d
}

/// `#<=>`: document (pre-order) position, so an array of nodes can be sorted.
///
/// nil when the nodes are not comparable: a non-node, an XML node, different
/// documents or detached subtrees with no common root, or an attribute node -
/// attributes are not in the `first_child`/`next` chain, so their order is not
/// defined here. Included via Comparable, which supplies `<`, `>`, `between?`
/// and the rest.
pub fn spaceship(ruby: &Ruby, rb_self: Value, other: Value) -> Value {
    unsafe {
        use magnus::rb_sys::AsRawValue;
        let nil = ruby.qnil().as_value();

        if !is_kind_of(other, mkr_cNode) {
            return nil;
        }
        /* An XML node is never order-comparable to an HTML one, and asking is
         * how we avoid unwrap's TypeError below. */
        if is_kind_of(
            Value::from_raw(crate::glue::abi::mkr_node_document(other.as_raw())),
            mkr_cXmlDocument,
        ) {
            return nil;
        }

        let a = unwrap(rb_self);
        let b = unwrap(other);
        if a == b {
            return ruby.integer_from_i64(0).as_value();
        }
        if (*a).type_ == ty::ATTRIBUTE
            || (*b).type_ == ty::ATTRIBUTE
            || (*a).owner_document != (*b).owner_document
        {
            return nil;
        }

        let (da, db) = (depth(a), depth(b));
        let mut pa = a;
        let mut pb = b;

        /* Raise the deeper node to the other's depth; landing ON the other
         * makes that other an ancestor, which comes first in pre-order. */
        if da > db {
            for _ in 0..(da - db) {
                pa = (*pa).parent;
            }
            if pa == b {
                return ruby.integer_from_i64(1).as_value();
            }
        } else if db > da {
            for _ in 0..(db - da) {
                pb = (*pb).parent;
            }
            if pb == a {
                return ruby.integer_from_i64(-1).as_value();
            }
        }

        /* Climb both until they share a parent (the lowest common ancestor). */
        while (*pa).parent != (*pb).parent {
            if (*pa).parent.is_null() || (*pb).parent.is_null() {
                return nil; /* different trees */
            }
            pa = (*pa).parent;
            pb = (*pb).parent;
        }
        if (*pa).parent.is_null() {
            return nil; /* two distinct roots */
        }

        /* pa and pb are distinct siblings: earlier in the child list is first. */
        let mut c = (*(*pa).parent).first_child;
        while !c.is_null() {
            if c == pa {
                return ruby.integer_from_i64(-1).as_value();
            }
            if c == pb {
                return ruby.integer_from_i64(1).as_value();
            }
            c = (*c).next;
        }
        nil /* unreachable for a well-formed tree */
    }
}
