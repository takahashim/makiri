//! The HTML node's mutators and the Document factories.
//!
//! The Ruby surface of each edit: reading and verifying the arguments, the
//! error each refusal raises, and the value handed back. The DOM's rules about
//! what may go where are [`crate::dom_rules::check`]'s, and every edit starts at
//! `bridge::html::edit` and reaches the document through its
//! `HtmlEdit::with_tree` (or its attribute and data twins), which drops the
//! document's indexes and lends the document by `&mut` - so no method here can
//! forget to, or read the document while it is changed. Each
//! method converts its arguments BETWEEN the two: a conversion can allocate,
//! and so raise, and must do so before the indexes are dropped. What touches a
//! raw handle or a String's bytes - the adopt copy, the fragment import, the
//! handoff of a verified String to the arena - is a primitive in
//! [`crate::bridge::html`], so this module holds no unsafe.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};

use crate::bridge::ruby::{makiri_error, string_arg};

use crate::bridge::html::fragment::{set_template_inner_html, stage_fragment_in};
use crate::bridge::html::{
    check_owning_doc, edit, insert, new_node, wrap_html_node, HtmlEdit, HtmlSelf,
};
use crate::bridge::string::{
    namespace_arg, optional, ruby_verified_data, ruby_verified_name, ruby_verified_text, HtmlSource,
};
use crate::dom_rules::{self, TextContent};
use crate::glue::dom_names::dom_extract as extract;
use crate::glue::node::DOCUMENT_NOT_REMOVABLE;
use crate::html_dom::html::{CreatePiError, HtmlDocMut, NodeType, Place, RawNode};
use crate::xml::dom_name;

/// `name` held to the WHATWG DOM rule `ok`, else `ArgumentError` - the DOM's
/// InvalidCharacterError, and what the XML side raises for a bad name.
///
/// Unchecked, a name was written into the markup as it stood:
/// `name = "img src=x onerror=alert(1)"` serialized as that tag with those
/// attributes, and `e['x="y" onload'] = v` as two attributes. Nokogiri's HTML5
/// does not check either; this is the DOM's rule, and XML already had its own.
fn check_dom_name(
    name: &crate::bridge::string::RubyText,
    ok: impl Fn(&[u8]) -> bool,
    what: &str,
) -> Result<(), Error> {
    if ok(name.as_bytes()) {
        return Ok(());
    }
    Err(Error::new(
        crate::bridge::rb::Builtin::ArgumentError,
        format_args!("invalid HTML {what} name"),
    ))
}

/// What an attribute setter raises on a node that is not an element.
const NOT_AN_ELEMENT: &str = "cannot set an attribute on a non-element node";

/// The receiver cleared for an edit ([`edit`]), when it is an element - asked
/// before any argument is converted, so a non-element answers without reading
/// one. `None` for any other node: each caller says what that means (a
/// refusal, or nothing to do).
fn element_edit<'a>(ruby: &Ruby, this: &'a mut HtmlSelf) -> Result<Option<HtmlEdit<'a>>, Error> {
    let edit = edit(ruby, this)?;
    Ok((edit.node_type()? == NodeType::Element).then_some(edit))
}

/// An attribute edit of the receiver as an element, once every argument is
/// converted. Its node type was checked before the conversion (an argument
/// cannot change it), so a non-element here is a broken invariant - raised as
/// one rather than assumed away. It leaves the tree version alone.
fn with_element<R>(
    edit: HtmlEdit<'_>,
    f: impl FnOnce(&mut HtmlDocMut<'_>, RawNode) -> R,
) -> Result<R, Error> {
    edit.with_attributes(|d, el| {
        if d.node(el).element().is_none() {
            return Err(crate::bridge::ruby::internal_error(
                "an attribute edit's checked element is not one",
            ));
        }
        Ok(f(d, el))
    })?
}

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// `node.add_child(child)` -> child.
pub fn add_child(ruby: &Ruby, mut this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    insert(ruby, &mut this, rb_child, Place::Child)
}

/// `node << child` -> node (chainable).
pub fn lshift(ruby: &Ruby, mut this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    insert(ruby, &mut this, rb_child, Place::Child)?;
    Ok(this.value)
}

/// `node.add_previous_sibling(node)` / `before` -> node.
pub fn before(ruby: &Ruby, mut this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    insert(ruby, &mut this, rb_node, Place::Before)
}

/// `node.add_next_sibling(node)` / `after` -> node.
pub fn after(ruby: &Ruby, mut this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    insert(ruby, &mut this, rb_node, Place::After)
}

/// `node.replace(other)` -> other.
pub fn replace(ruby: &Ruby, mut this: HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    insert(ruby, &mut this, rb_other, Place::Replace)
}

/// `node.remove` / `node.unlink` -> node.
pub fn remove(ruby: &Ruby, mut this: HtmlSelf) -> Result<Value, Error> {
    if !dom_rules::removable(this.read()?.node().node_type()) {
        return Err(makiri_error(DOCUMENT_NOT_REMOVABLE));
    }
    let edit = edit(ruby, &mut this)?;
    /* An Attr leaves its element's attribute list: an attribute edit,
     * which changes no child list - as the XML side's `remove` is. */
    if edit.node_type()? == NodeType::Attribute {
        edit.with_attributes(|d, attr| d.remove_from_owner(attr))?;
        return Ok(this.value);
    }
    edit.with_tree(|d, node| {
        if d.node(node).parent().is_some() {
            d.detach(node);
        }
    })?;
    Ok(this.value)
}

/* ------------------------------------------------------------------ *
 * attribute and content mutation                                     *
 * ------------------------------------------------------------------ */

/// `element[name] = value` -> value.
pub fn aset(
    ruby: &Ruby,
    mut this: HtmlSelf,
    rb_name: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    let Some(edit) = element_edit(ruby, &mut this)? else {
        return Err(makiri_error(NOT_AN_ELEMENT));
    };
    let nv = ruby_verified_name(ruby, rb_name, "attribute name")?;
    let vv = ruby_verified_data(ruby, rb_value, "attribute value")?;
    check_dom_name(&nv, dom_name::valid_attribute_local_name, "attribute")?;
    with_element(edit, |d, el| {
        crate::bridge::html::set_attribute(d, el, &nv, &vv)
    })?
    .map_err(|_| makiri_error("failed to set attribute"))?;
    Ok(rb_value)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    ruby: &Ruby,
    mut this: HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    let Some(edit) = element_edit(ruby, &mut this)? else {
        return Err(makiri_error(NOT_AN_ELEMENT));
    };
    let qv = ruby_verified_name(ruby, rb_qname, "attribute qualified name")?;
    let vv = ruby_verified_data(ruby, rb_value, "attribute value")?;
    let nv = namespace_arg(ruby, rb_ns, "namespace")?;
    /* The DOM's "validate and extract". It named `(nil, "x:y")` a
     * prefixed attribute in no namespace. */
    let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
    extract(
        ns,
        qv.as_bytes(),
        dom_name::valid_attribute_local_name,
        "HTML attribute",
    )?;
    let ns = nv.as_ref().map(|n| n.as_bytes());
    with_element(edit, |d, el| {
        crate::bridge::html::set_attribute_ns(d, el, ns, &qv, &vv)
    })?
    .map_err(|_| makiri_error("failed to set namespaced attribute"))?;
    Ok(rb_value)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    mut this: HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    let Some(edit) = element_edit(ruby, &mut this)? else {
        return Ok(ruby.qnil().as_value());
    };
    let lv = ruby_verified_text(ruby, rb_local, "attribute local name")?;
    let nv = namespace_arg(ruby, rb_ns, "namespace")?;
    let ns = nv.as_ref().map(|n| n.as_bytes());
    with_element(edit, |d, el| {
        crate::bridge::html::remove_attribute_ns(d, el, ns, &lv)
    })?;
    Ok(ruby.qnil().as_value())
}

/// `node.content = text` -> text.
pub fn set_content(ruby: &Ruby, mut this: HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    let edit = edit(ruby, &mut this)?;
    let tv = ruby_verified_data(ruby, rb_text, "node content")?;
    /* An Attr's content is its value, an attribute edit; a Text, Comment,
     * CDATA or PI node's is its data. Neither changes a child list. A
     * Document's or DocumentType's is nothing, so no edit is recorded. */
    let set = |d: &mut HtmlDocMut<'_>, n: RawNode| crate::bridge::html::set_text_content(d, n, &tv);
    match dom_rules::text_content(edit.node_type()?) {
        TextContent::AttrValue => edit.with_attributes(set)?,
        TextContent::Data => edit.with_data(set)?,
        TextContent::ReplaceChildren => edit.with_tree(set)?,
        TextContent::Nothing => return Ok(rb_text),
    }
    .map_err(|_| makiri_error("failed to set node content"))?;
    Ok(rb_text)
}

/// `element.delete(name)` -> self.
pub fn delete(ruby: &Ruby, mut this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let Some(edit) = element_edit(ruby, &mut this)? else {
        return Ok(rb_self);
    };
    let nv = ruby_verified_text(ruby, rb_name, "attribute name")?;
    with_element(edit, |d, el| {
        crate::bridge::html::remove_attribute(d, el, &nv)
    })?;
    Ok(rb_self)
}

/// `element.inner_html = html` -> html.
///
/// All or nothing: the new content is parsed and imported into a detached
/// fragment first, and only then are the old children swapped for it.
pub fn set_inner_html(ruby: &Ruby, mut this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    let Some(edit) = element_edit(ruby, &mut this)? else {
        return Err(makiri_error("inner_html= requires an element"));
    };
    /* A String only, transcoded before the edit - nothing inside it may
     * run Ruby. */
    let src = HtmlSource::from_ruby(ruby, string_arg(rb_html)?)?;
    edit.with_tree(|d, node| {
        /* WHATWG: a `<template>`'s inner HTML is its contents fragment, not
         * the element's (empty) children - the same node
         * `Element#content_fragment` exposes and `#to_html` serializes, so
         * `inner_html`/`inner_html=`/`to_html` all agree. */
        match d.node(node).template_content().map(RawNode::from) {
            Some(content) => set_template_inner_html(d, node, content, &src),
            None => {
                let staged = stage_fragment_in(d, node, &src)?;
                /* Detached, not destroyed: the arena reclaims them with the
                 * document. */
                d.detach_children(node);
                d.place(node, staged, Place::Child);
                Ok(())
            }
        }
    })??;
    Ok(rb_html)
}

/// `node.outer_html = html` -> html. All or nothing, as `inner_html=`.
///
/// The parent is looked up inside the edit, where the tree cannot change
/// under it. (When an argument's `#to_s` ran first, one that removed the
/// receiver left a parent read earlier stale, and the new content went
/// nowhere.)
pub fn set_outer_html(ruby: &Ruby, mut this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    let edit = edit(ruby, &mut this)?;
    /* A String only, transcoded before the edit - nothing inside it may
     * run Ruby. */
    let src = HtmlSource::from_ruby(ruby, string_arg(rb_html)?)?;
    edit.with_tree(|d, node| {
        /* An attribute's `parent()` is its owner element, but it is no child
         * of it: the replace below would drop the new content and report
         * success. */
        let n = d.node(node);
        let Some(parent) = n
            .parent()
            .filter(|_| n.node_type() != NodeType::Attribute)
            .filter(|p| p.node_type() == NodeType::Element)
            .map(RawNode::from)
        else {
            return Err(makiri_error(
                "outer_html= requires a node with a parent element",
            ));
        };
        let staged = stage_fragment_in(d, parent, &src)?;
        d.place(node, staged, Place::Replace);
        Ok(())
    })??;
    Ok(rb_html)
}

/* ------------------------------------------------------------------ *
 * node creation (Document)                                           *
 * ------------------------------------------------------------------ */

/// A node the factory made, or the error naming what failed.
///
/// A failed wrap (`NoMemoryError`) is `Err`, and the factory's argument copies
/// drop with its frame.
fn created(ruby: &Ruby, node: Option<RawNode>, rb_self: Value, what: &str) -> Result<Value, Error> {
    match node {
        Some(n) => wrap_html_node(ruby, n, rb_self),
        None => Err(makiri_error(format_args!("failed to create {what}"))),
    }
}

/// `Document#create_element(name)` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    let nv = ruby_verified_name(ruby, rb_name, "element name")?;
    check_dom_name(&nv, dom_name::valid_element_local_name, "element")?;
    created(
        ruby,
        new_node(ruby, &rb_self, |d| d.create_element(nv.as_bytes()))?,
        rb_self,
        "element",
    )
}

/// `Document#create_element_ns(namespace_uri, qualified_name)` -> Element.
///
/// The DOM's createElementNS: the name is split at its first colon and each
/// half held to the DOM's rule (`ArgumentError`), then the namespace to the
/// name (`Makiri::Error`). Not `create_element`'s rule - `"0:a"` is prefix `0`
/// and local name `a` - and the name keeps its case, as a parsed foreign
/// element's does.
pub fn create_element_ns(
    ruby: &Ruby,
    rb_self: Value,
    rb_ns: Value,
    rb_qname: Value,
) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    let nv = namespace_arg(ruby, rb_ns, "namespace")?;
    let qv = ruby_verified_name(ruby, rb_qname, "element qualified name")?;
    let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
    let (prefix, local) = extract(
        ns,
        qv.as_bytes(),
        dom_name::valid_element_local_name,
        "HTML element",
    )?;
    created(
        ruby,
        new_node(ruby, &rb_self, |d| d.create_element_ns(local, ns, prefix))?,
        rb_self,
        "element",
    )
}

/// `create_loose_dom_element(qualified_name, prefix, local_name, namespace_uri)`
/// -> Element.
///
/// The XML Document's factory of the same name: the name is not parsed, the
/// caller gives its split, so a local name keeps its colons (`"foo:"`) and its
/// case. Neither [`create_element`] (lower-cased, in the HTML namespace) nor
/// [`create_element_ns`] (split at the first colon) can make that. The names
/// are held to the DOM's rule (`ArgumentError`); the namespace is not checked
/// against the prefix, as on XML.
pub fn create_loose_dom_element(
    ruby: &Ruby,
    rb_self: Value,
    rb_qname: Value,
    rb_prefix: Value,
    rb_local: Value,
    rb_ns: Value,
) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    let qv = ruby_verified_name(ruby, rb_qname, "qualified name")?;
    let lv = ruby_verified_name(ruby, rb_local, "local name")?;
    let pv = if rb_prefix.is_nil() {
        None
    } else {
        Some(ruby_verified_name(ruby, rb_prefix, "prefix")?)
    };
    let nv = namespace_arg(ruby, rb_ns, "namespace URI")?;
    let prefix = pv.as_ref().map(|p| p.as_bytes());
    dom_name::split_loose_dom_name(qv.as_bytes(), prefix, lv.as_bytes())
        .map_err(|e| Error::new(crate::bridge::rb::Builtin::ArgumentError, e.message()))?;
    let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
    created(
        ruby,
        new_node(ruby, &rb_self, |d| {
            d.create_element_ns(lv.as_bytes(), ns, prefix.unwrap_or_default())
        })?,
        rb_self,
        "element",
    )
}

/// `Document#create_text_node(content)` -> Text.
pub fn create_text_node(ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    let tv = ruby_verified_data(ruby, rb_text, "text content")?;
    created(
        ruby,
        new_node(ruby, &rb_self, |d| d.create_text(tv.as_bytes()))?,
        rb_self,
        "text node",
    )
}

/// `Document#create_comment(content)` -> Comment.
pub fn create_comment(ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    let tv = ruby_verified_data(ruby, rb_text, "comment content")?;
    created(
        ruby,
        new_node(ruby, &rb_self, |d| d.create_comment(tv.as_bytes()))?,
        rb_self,
        "comment",
    )
}

/// `Document#create_processing_instruction(target, data)` -> PI.
pub fn create_pi(
    ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    let tv = ruby_verified_name(ruby, rb_target, "processing instruction target")?;
    let dv = ruby_verified_text(ruby, rb_data, "processing instruction data")?;
    /* DOM createProcessingInstruction: the target must match the XML Name
     * production. The arena's factory checks only the data (below), and
     * an unchecked target is written into the output as it is - markup
     * included. */
    check_dom_name(
        &tv,
        crate::xml::chars::validate_name,
        "processing instruction target",
    )?;
    /* ... and the data must not hold `?>` - the factory's rule. */
    match new_node(ruby, &rb_self, |d| {
        d.create_pi(tv.as_bytes(), dv.as_bytes())
    })? {
        Ok(n) => created(ruby, Some(n), rb_self, "processing instruction"),
        Err(CreatePiError::ClosingInData) => Err(Error::new(
            crate::bridge::rb::Builtin::ArgumentError,
            CreatePiError::MESSAGE,
        )),
        Err(CreatePiError::Oom) => created(ruby, None, rb_self, "processing instruction"),
    }
}

/// `Document#create_document_type(name, public_id = "", system_id = "")`.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let args = crate::bridge::args::scan(ruby, args, crate::bridge::args::Shape::positional(1, 2))?;
    let rb_name = args.required(0);
    let (rb_pub, rb_sys_) = (args.optional(0), args.optional(1));

    check_owning_doc(&rb_self)?;
    let nv = ruby_verified_name(ruby, rb_name, "doctype name")?;
    if !crate::bridge::html::valid_doctype_name(&nv) {
        /* The caller's error, so the exception class is picked here - the
         * check itself is the DOM layer's. */
        return Err(Error::new(
            crate::bridge::rb::Builtin::ArgumentError,
            "invalid doctype name",
        ));
    }

    let verified = |v: Option<Value>, what: &'static str| match v {
        Some(v) => optional(v, |v| ruby_verified_text(ruby, v, what)),
        None => Ok(None),
    };
    let pv = verified(rb_pub, "doctype public id")?;
    let sv = verified(rb_sys_, "doctype system id")?;
    created(
        ruby,
        new_node(ruby, &rb_self, |d| {
            d.create_doctype(
                nv.as_bytes(),
                pv.as_ref().map(|v| v.as_bytes()),
                sv.as_ref().map(|v| v.as_bytes()),
            )
        })?,
        rb_self,
        "doctype",
    )
}

/// `Document#create_document_fragment` -> an EMPTY DocumentFragment.
pub fn create_document_fragment(ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    check_owning_doc(&rb_self)?;
    created(
        ruby,
        new_node(ruby, &rb_self, |d| d.create_fragment())?,
        rb_self,
        "document fragment",
    )
}
