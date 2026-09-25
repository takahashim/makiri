//! The HTML node's mutators and the Document factories.
//!
//! The Ruby surface of each edit: reading and verifying the arguments, the
//! error each refusal raises, and the value handed back. The DOM's rules about
//! what may go where are the adapter's `Insertion`, and every edit starts at
//! `bridge::html::edit` and reaches the tree through its `HtmlEdit::node`,
//! which drops the document's indexes - so no method here can forget to. Each
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
use crate::bridge::html::{edit, insert, owning_doc, wrap_html_node, HtmlEdit, HtmlSelf};
use crate::bridge::string::{ruby_verified_data, ruby_verified_text, ruby_verified_text_opt};
use crate::lexbor::adapter::html::{HtmlElementMut, NodeType, Place, RawNode};
use crate::xml::dom_name;
use crate::xml::qname::Split;

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
    if ok(name.as_verified().as_bytes()) {
        return Ok(());
    }
    Err(Error::new(
        ruby.exception_arg_error(),
        format!("invalid HTML {what} name"),
    ))
}

/// The receiver as an element, once every argument is converted. Its node type
/// was checked before the conversion (an argument cannot change it), so the
/// `None` arm is unreachable - it answers `refusal` rather than assuming so.
fn element_of<'a>(edit: HtmlEdit<'a>, refusal: &'static str) -> Result<HtmlElementMut<'a>, Error> {
    edit.node()?
        .element_mut()
        .ok_or_else(|| makiri_error(refusal))
}

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// `node.add_child(child)` -> child.
pub fn add_child(_ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&this, rb_child, Place::Child))
}

/// `node << child` -> node (chainable).
pub fn lshift(_ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        insert(&this, rb_child, Place::Child)?;
        Ok(this.value)
    })
}

/// `node.add_previous_sibling(node)` / `before` -> node.
pub fn before(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&this, rb_node, Place::Before))
}

/// `node.add_next_sibling(node)` / `after` -> node.
pub fn after(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&this, rb_node, Place::After))
}

/// `node.replace(other)` -> other.
pub fn replace(_ruby: &Ruby, this: HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(&this, rb_other, Place::Replace))
}

/// `node.remove` / `node.unlink` -> node.
pub fn remove(_ruby: &Ruby, this: HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let node = edit(&this)?.node()?;
        if node.node().node_type() == NodeType::Attribute {
            return Err(makiri_error("use delete(name) to remove an attribute"));
        }
        if node.parent().is_some() {
            node.detach();
        }
        Ok(this.value)
    })
}

/* ------------------------------------------------------------------ *
 * attribute and content mutation                                     *
 * ------------------------------------------------------------------ */

/// `element[name] = value` -> value.
pub fn aset(ruby: &Ruby, this: HtmlSelf, rb_name: Value, rb_value: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "cannot set an attribute on a non-element node";
        let edit = edit(&this)?;
        if edit.node_type() != NodeType::Element {
            return Err(makiri_error(REFUSAL));
        }
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        let vv = ruby_verified_data(rb_value, "attribute value")?;
        check_dom_name(ruby, &nv, dom_name::valid_attribute_local_name, "attribute")?;
        let el = element_of(edit, REFUSAL)?;
        if !crate::bridge::html::set_attribute(el, &nv, &vv) {
            return Err(makiri_error("failed to set attribute"));
        }
        Ok(rb_value)
    })
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "cannot set an attribute on a non-element node";
        let edit = edit(&this)?;
        if edit.node_type() != NodeType::Element {
            return Err(makiri_error(REFUSAL));
        }
        let qv = ruby_verified_text(rb_qname, "attribute qualified name")?;
        let vv = ruby_verified_data(rb_value, "attribute value")?;
        let nv = ruby_verified_text_opt(rb_ns, "namespace")?;
        /* The DOM's "validate and extract": split at the first colon, check
         * both halves, then that the namespace fits them - the rule XML's
         * set_attribute_ns applies too (`xml::qname::ns_fits_name`). It named
         * `(nil, "x:y")` a prefixed attribute in no namespace. */
        let q = qv.as_verified().as_bytes();
        let colon = q.iter().position(|&b| b == b':');
        let (prefix, local) = match colon {
            Some(i) => (&q[..i], &q[i + 1..]),
            None => (&b""[..], q),
        };
        let names_ok = dom_name::valid_attribute_local_name(local)
            && (colon.is_none() || dom_name::valid_namespace_prefix(prefix));
        check_dom_name(ruby, &qv, |_| names_ok, "attribute")?;
        let ns = nv.as_ref().map_or(&b""[..], |n| n.as_verified().as_bytes());
        let split = match colon {
            Some(i) => Split::prefixed(i as u32, (q.len() - i - 1) as u32),
            None => Split::unprefixed(q.len() as u32),
        };
        if !crate::xml::qname::ns_fits_name(ns, q, &split) {
            return Err(makiri_error(
                "the namespace does not fit the qualified name (a prefix needs a namespace; \
xml and xmlns take only their own)",
            ));
        }
        let el = element_of(edit, REFUSAL)?;
        if !crate::bridge::html::set_attribute_ns(el, nv.as_ref(), &qv, &vv) {
            return Err(makiri_error("failed to set namespaced attribute"));
        }
        Ok(rb_value)
    })
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&this)?;
        if edit.node_type() != NodeType::Element {
            return Ok(ruby.qnil().as_value());
        }
        let lv = ruby_verified_text(rb_local, "attribute local name")?;
        let nv = ruby_verified_text_opt(rb_ns, "namespace")?;
        let el = element_of(edit, "remove_attribute_ns requires an element")?;
        crate::bridge::html::remove_attribute_ns(el, nv.as_ref(), &lv);
        Ok(ruby.qnil().as_value())
    })
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, this: HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&this)?;
        let tv = ruby_verified_data(rb_text, "node content")?;
        let node = edit.node()?;
        if !crate::bridge::html::set_text_content(node, &tv) {
            return Err(makiri_error("failed to set node content"));
        }
        Ok(rb_text)
    })
}

/// `element.delete(name)` -> self.
pub fn delete(_ruby: &Ruby, this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let rb_self = this.value;
        let edit = edit(&this)?;
        if edit.node_type() != NodeType::Element {
            return Ok(rb_self);
        }
        let nv = ruby_verified_text(rb_name, "attribute name")?;
        let el = element_of(edit, "delete requires an element")?;
        crate::bridge::html::remove_attribute(el, &nv);
        Ok(rb_self)
    })
}

/// `element.inner_html = html` -> html.
///
/// All or nothing: the new content is parsed and imported into a detached
/// fragment first, and only then are the old children swapped for it.
pub fn set_inner_html(_ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&this)?;
        if edit.node_type() != NodeType::Element {
            return Err(makiri_error("inner_html= requires an element"));
        }
        /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
        let html = string_of(rb_html)?;
        let node = edit.node()?;
        /* WHATWG: a `<template>`'s inner HTML is its contents fragment, not the
         * element's (empty) children - the same node `Element#content_fragment`
         * exposes and `#to_html` serializes, so `inner_html`/`inner_html=`/
         * `to_html` all agree. */
        if let Some(content) = node.template_content_mut() {
            set_template_inner_html(node, content, html)?;
        } else {
            let staged = stage_fragment_in(node, html)?;
            /* Detached, not destroyed: the arena reclaims them with the document. */
            while let Some(c) = node.first_child() {
                c.detach();
            }
            node.place(staged, Place::Child);
        }
        Ok(rb_html)
    })
}

/// `node.outer_html = html` -> html. All or nothing, as `inner_html=`.
///
/// The parent is looked up AFTER the argument is converted: its `#to_s` may
/// have moved or removed the receiver, and a parent read before it was a
/// parent the receiver no longer had - the new content went nowhere and the
/// call still reported success.
pub fn set_outer_html(_ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&this)?;
        /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
        let html = string_of(rb_html)?;
        let node = edit.node()?;
        let Some(parent) = node
            .parent()
            .filter(|p| p.node().node_type() == NodeType::Element)
        else {
            return Err(makiri_error(
                "outer_html= requires a node with a parent element",
            ));
        };
        let staged = stage_fragment_in(parent, html)?;
        node.place(staged, Place::Replace);
        Ok(rb_html)
    })
}

/* ------------------------------------------------------------------ *
 * node creation (Document)                                           *
 * ------------------------------------------------------------------ */

/// A node the factory made, or the error naming what failed.
fn created(node: Option<RawNode>, rb_self: Value, what: &str) -> Result<Value, Error> {
    match node {
        Some(n) => Ok(wrap_html_node(n, rb_self)),
        None => Err(makiri_error(format!("failed to create {what}"))),
    }
}

/// `Document#create_element(name)` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = owning_doc(&rb_self)?;
        let nv = ruby_verified_text(rb_name, "element name")?;
        check_dom_name(ruby, &nv, dom_name::valid_element_local_name, "element")?;
        created(
            crate::bridge::html::create_element(doc, &nv),
            rb_self,
            "element",
        )
    })
}

/// `Document#create_text_node(content)` -> Text.
pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = owning_doc(&rb_self)?;
        let tv = ruby_verified_data(rb_text, "text content")?;
        created(
            crate::bridge::html::create_text(doc, &tv),
            rb_self,
            "text node",
        )
    })
}

/// `Document#create_comment(content)` -> Comment.
pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = owning_doc(&rb_self)?;
        let tv = ruby_verified_data(rb_text, "comment content")?;
        created(
            crate::bridge::html::create_comment(doc, &tv),
            rb_self,
            "comment",
        )
    })
}

/// `Document#create_processing_instruction(target, data)` -> PI.
pub fn create_pi(
    _ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = owning_doc(&rb_self)?;
        let tv = ruby_verified_text(rb_target, "processing instruction target")?;
        let dv = ruby_verified_text(rb_data, "processing instruction data")?;
        created(
            crate::bridge::html::create_pi(doc, &tv, &dv),
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

        let doc = owning_doc(&rb_self)?;
        let nv = ruby_verified_text(rb_name, "doctype name")?;
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
            crate::bridge::html::create_doctype(doc, &nv, pv.as_ref(), sv.as_ref()),
            rb_self,
            "doctype",
        )
    })
}

/// `Document#create_document_fragment` -> an EMPTY DocumentFragment.
pub fn create_document_fragment(_ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = owning_doc(&rb_self)?;
        created(
            doc.create_fragment().map(RawNode::from),
            rb_self,
            "document fragment",
        )
    })
}
