//! The HTML node's mutators and the Document factories.
//!
//! The Ruby surface of each edit: reading and verifying the arguments, the
//! error each refusal raises, and the value handed back. The DOM's rules about
//! what may go where are the adapter's `Insertion`, and every edit starts at
//! `bridge::html::edit` and reaches the document through its
//! `HtmlEdit::with_tree` (or its attribute and data twins), which drops the
//! document's indexes and lends the document by `&mut` - so no method here can
//! forget to, or read the document while it is changed. Each
//! method converts its arguments BETWEEN the two: a conversion is the
//! argument's `#to_s`, and a query there must not rebuild the indexes from the
//! tree the edit is about to change. What touches a
//! raw handle or a String's bytes - the adopt copy, the fragment import, the
//! handoff of a verified String to Lexbor - is a primitive in
//! [`crate::bridge::html`], so this module holds no unsafe.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, Ruby, Value};

use crate::bridge::ruby::{makiri_error, string_of};

use crate::bridge::fragment::{set_template_inner_html, stage_fragment_in};
use crate::bridge::html::{
    check_owning_doc, edit, insert, new_node, wrap_html_node, HtmlEdit, HtmlSelf,
};
use crate::bridge::string::{
    namespace_arg, ruby_verified_data, ruby_verified_name, ruby_verified_text,
    ruby_verified_text_opt, HtmlSource,
};
use crate::dom_rules::{self, TextContent};
use crate::glue::node::{dom_extract as extract, DOCUMENT_NOT_REMOVABLE};
use crate::html_dom::html::{HtmlDocMut, NodeType, Place, RawNode};
use crate::xml::dom_name;

/// `name` held to the WHATWG DOM rule `ok`, else `ArgumentError` - the DOM's
/// InvalidCharacterError, and what the XML side raises for a bad name.
///
/// Unchecked, a name was written into the markup as it stood:
/// `name = "img src=x onerror=alert(1)"` serialized as that tag with those
/// attributes, and `e['x="y" onload'] = v` as two attributes. Nokogiri's HTML5
/// does not check either; this is the DOM's rule, and XML already had its own.
fn check_dom_name(
    ruby: &Ruby,
    name: &crate::bridge::string::RubyText,
    ok: impl Fn(&[u8]) -> bool,
    what: &str,
) -> Result<(), Error> {
    if ok(name.as_bytes()) {
        return Ok(());
    }
    Err(Error::new(
        ruby.exception_arg_error(),
        format!("invalid HTML {what} name"),
    ))
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
pub fn add_child(_ruby: &Ruby, mut this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&mut this, rb_child, Place::Child))
}

/// `node << child` -> node (chainable).
pub fn lshift(_ruby: &Ruby, mut this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        insert(&mut this, rb_child, Place::Child)?;
        Ok(this.value)
    })
}

/// `node.add_previous_sibling(node)` / `before` -> node.
pub fn before(_ruby: &Ruby, mut this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&mut this, rb_node, Place::Before))
}

/// `node.add_next_sibling(node)` / `after` -> node.
pub fn after(_ruby: &Ruby, mut this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&mut this, rb_node, Place::After))
}

/// `node.replace(other)` -> other.
pub fn replace(_ruby: &Ruby, mut this: HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&mut this, rb_other, Place::Replace))
}

/// `node.remove` / `node.unlink` -> node.
pub fn remove(_ruby: &Ruby, mut this: HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        if !dom_rules::removable(this.node().node_type()) {
            return Err(makiri_error(DOCUMENT_NOT_REMOVABLE));
        }
        let edit = edit(&mut this)?;
        /* An Attr leaves its element's attribute list: an attribute edit,
         * which changes no child list - as the XML side's `remove` is. */
        if edit.node_type() == NodeType::Attribute {
            edit.with_attributes(|d, attr| d.remove_from_owner(attr))?;
            return Ok(this.value);
        }
        edit.with_tree(|d, node| {
            if d.node(node).parent().is_some() {
                d.detach(node);
            }
        })?;
        Ok(this.value)
    })
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
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "cannot set an attribute on a non-element node";
        let edit = edit(&mut this)?;
        if edit.node_type() != NodeType::Element {
            return Err(makiri_error(REFUSAL));
        }
        let nv = ruby_verified_name(rb_name, "attribute name")?;
        let vv = ruby_verified_data(rb_value, "attribute value")?;
        check_dom_name(ruby, &nv, dom_name::valid_attribute_local_name, "attribute")?;
        with_element(edit, |d, el| {
            crate::bridge::html::set_attribute(d, el, &nv, &vv)
        })?
        .map_err(|_| makiri_error("failed to set attribute"))?;
        Ok(rb_value)
    })
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    ruby: &Ruby,
    mut this: HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "cannot set an attribute on a non-element node";
        let edit = edit(&mut this)?;
        if edit.node_type() != NodeType::Element {
            return Err(makiri_error(REFUSAL));
        }
        let qv = ruby_verified_name(rb_qname, "attribute qualified name")?;
        let vv = ruby_verified_data(rb_value, "attribute value")?;
        let nv = namespace_arg(rb_ns, "namespace")?;
        /* The DOM's "validate and extract". It named `(nil, "x:y")` a
         * prefixed attribute in no namespace. */
        let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
        extract(
            ruby,
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
    })
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    mut this: HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&mut this)?;
        if edit.node_type() != NodeType::Element {
            return Ok(ruby.qnil().as_value());
        }
        let lv = ruby_verified_text(rb_local, "attribute local name")?;
        let nv = namespace_arg(rb_ns, "namespace")?;
        let ns = nv.as_ref().map(|n| n.as_bytes());
        with_element(edit, |d, el| {
            crate::bridge::html::remove_attribute_ns(d, el, ns, &lv)
        })?;
        Ok(ruby.qnil().as_value())
    })
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, mut this: HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&mut this)?;
        let tv = ruby_verified_data(rb_text, "node content")?;
        /* An Attr's content is its value, an attribute edit; a Text, Comment,
         * CDATA or PI node's is its data. Neither changes a child list. A
         * Document's or DocumentType's is nothing, so no edit is recorded. */
        let set =
            |d: &mut HtmlDocMut<'_>, n: RawNode| crate::bridge::html::set_text_content(d, n, &tv);
        match dom_rules::text_content(edit.node_type()) {
            TextContent::AttrValue => edit.with_attributes(set)?,
            TextContent::Data => edit.with_data(set)?,
            TextContent::ReplaceChildren => edit.with_tree(set)?,
            TextContent::Nothing => return Ok(rb_text),
        }
        .map_err(|_| makiri_error("failed to set node content"))?;
        Ok(rb_text)
    })
}

/// `element.delete(name)` -> self.
pub fn delete(_ruby: &Ruby, mut this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let rb_self = this.value;
        let edit = edit(&mut this)?;
        if edit.node_type() != NodeType::Element {
            return Ok(rb_self);
        }
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        with_element(edit, |d, el| {
            crate::bridge::html::remove_attribute(d, el, &nv)
        })?;
        Ok(rb_self)
    })
}

/// `element.inner_html = html` -> html.
///
/// All or nothing: the new content is parsed and imported into a detached
/// fragment first, and only then are the old children swapped for it.
pub fn set_inner_html(_ruby: &Ruby, mut this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&mut this)?;
        if edit.node_type() != NodeType::Element {
            return Err(makiri_error("inner_html= requires an element"));
        }
        /* `to_str`/`to_s` is Ruby code that may raise: converted under protect,
         * and transcoded, before the edit - nothing inside it may run Ruby. */
        let src = HtmlSource::from_ruby(string_of(rb_html)?)?;
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
    })
}

/// `node.outer_html = html` -> html. All or nothing, as `inner_html=`.
///
/// The parent is looked up AFTER the argument is converted: its `#to_s` may
/// have moved or removed the receiver, and a parent read before it was a
/// parent the receiver no longer had - the new content went nowhere and the
/// call still reported success.
pub fn set_outer_html(_ruby: &Ruby, mut this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&mut this)?;
        /* `to_str`/`to_s` is Ruby code that may raise: converted under protect,
         * and transcoded, before the edit - nothing inside it may run Ruby. */
        let src = HtmlSource::from_ruby(string_of(rb_html)?)?;
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
    })
}

/* ------------------------------------------------------------------ *
 * node creation (Document)                                           *
 * ------------------------------------------------------------------ */

/// A node the factory made, or the error naming what failed.
///
/// The wrap is protected: every factory still owns its arguments' copies
/// here, and the wrapper's allocation raises `NoMemoryError` past this frame.
fn created(node: Option<RawNode>, rb_self: Value, what: &str) -> Result<Value, Error> {
    match node {
        Some(n) => crate::bridge::ruby::protected(|| wrap_html_node(n, rb_self)),
        None => Err(makiri_error(format!("failed to create {what}"))),
    }
}

/// `Document#create_element(name)` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        let nv = ruby_verified_name(rb_name, "element name")?;
        check_dom_name(ruby, &nv, dom_name::valid_element_local_name, "element")?;
        created(
            new_node(&rb_self, |d| d.create_element(nv.as_bytes()))?,
            rb_self,
            "element",
        )
    })
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
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        let nv = namespace_arg(rb_ns, "namespace")?;
        let qv = ruby_verified_name(rb_qname, "element qualified name")?;
        let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
        let (prefix, local) = extract(
            ruby,
            ns,
            qv.as_bytes(),
            dom_name::valid_element_local_name,
            "HTML element",
        )?;
        created(
            new_node(&rb_self, |d| d.create_element_ns(local, ns, prefix))?,
            rb_self,
            "element",
        )
    })
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
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        let qv = ruby_verified_name(rb_qname, "qualified name")?;
        let lv = ruby_verified_name(rb_local, "local name")?;
        let pv = if rb_prefix.is_nil() {
            None
        } else {
            Some(ruby_verified_name(rb_prefix, "prefix")?)
        };
        let nv = namespace_arg(rb_ns, "namespace URI")?;
        let prefix = pv.as_ref().map(|p| p.as_bytes());
        dom_name::split_loose_dom_name(qv.as_bytes(), prefix, lv.as_bytes())
            .map_err(|e| Error::new(ruby.exception_arg_error(), e.message()))?;
        let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
        created(
            new_node(&rb_self, |d| {
                d.create_element_ns(lv.as_bytes(), ns, prefix.unwrap_or_default())
            })?,
            rb_self,
            "element",
        )
    })
}

/// `Document#create_text_node(content)` -> Text.
pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        let tv = ruby_verified_data(rb_text, "text content")?;
        created(
            new_node(&rb_self, |d| d.create_text(tv.as_bytes()))?,
            rb_self,
            "text node",
        )
    })
}

/// `Document#create_comment(content)` -> Comment.
pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        let tv = ruby_verified_data(rb_text, "comment content")?;
        created(
            new_node(&rb_self, |d| d.create_comment(tv.as_bytes()))?,
            rb_self,
            "comment",
        )
    })
}

/// `Document#create_processing_instruction(target, data)` -> PI.
pub fn create_pi(
    ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        let tv = ruby_verified_name(rb_target, "processing instruction target")?;
        let dv = ruby_verified_text(rb_data, "processing instruction data")?;
        /* DOM createProcessingInstruction: the target must match the XML Name
         * production. Lexbor leaves that check as a TODO, and an unchecked
         * target is written into the output as it is - markup included. */
        check_dom_name(
            ruby,
            &tv,
            crate::xml::chars::validate_name,
            "processing instruction target",
        )?;
        /* ... and the data must not hold `?>`, which Lexbor refused only as a
         * failed creation. The message is the XML side's. */
        if dv.as_bytes().windows(2).any(|w| w == b"?>") {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "processing instruction data must not contain ?>",
            ));
        }
        created(
            new_node(&rb_self, |d| d.create_pi(tv.as_bytes(), dv.as_bytes()))?,
            rb_self,
            "processing instruction",
        )
    })
}

/// `Document#create_document_type(name, public_id = "", system_id = "")`.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let args = magnus::scan_args::scan_args::<
            (Value,),
            (Option<Value>, Option<Value>),
            (),
            (),
            (),
            (),
        >(args)?;
        let (rb_name,) = args.required;
        let (rb_pub, rb_sys_) = args.optional;

        check_owning_doc(&rb_self)?;
        let nv = ruby_verified_name(rb_name, "doctype name")?;
        if !crate::bridge::html::valid_doctype_name(&nv) {
            /* The caller's error, not Lexbor's, so the exception class is picked
             * here - the check itself is the DOM layer's. */
            return Err(Error::new(
                ruby.exception_arg_error(),
                "invalid doctype name",
            ));
        }

        let verified = |v: Option<Value>, what: &'static str| match v {
            Some(v) => ruby_verified_text_opt(v, what),
            None => Ok(None),
        };
        let pv = verified(rb_pub, "doctype public id")?;
        let sv = verified(rb_sys_, "doctype system id")?;
        created(
            new_node(&rb_self, |d| {
                d.create_doctype(
                    nv.as_bytes(),
                    pv.as_ref().map(|v| v.as_bytes()),
                    sv.as_ref().map(|v| v.as_bytes()),
                )
            })?,
            rb_self,
            "doctype",
        )
    })
}

/// `Document#create_document_fragment` -> an EMPTY DocumentFragment.
pub fn create_document_fragment(_ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        check_owning_doc(&rb_self)?;
        created(
            new_node(&rb_self, |d| d.create_fragment())?,
            rb_self,
            "document fragment",
        )
    })
}
