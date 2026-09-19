//! The HTML node's mutators and the Document factories (glue/ruby_html_mutate.c).
//!
//! The Ruby surface of each edit: reading and verifying the arguments, the
//! DOM's rules about what may go where, the error each refusal raises,
//! dropping the document's indexes, and the value handed back. What touches a
//! raw handle or a String's bytes - the adopt copy, the fragment import, the
//! handoff of a verified String to Lexbor - is a primitive in
//! [`crate::bridge::html`], so this module holds no unsafe.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, Ruby, Value};

use crate::bridge::ruby::makiri_error;

use crate::bridge::fragment::{parse_fragment_in, splice_fragment, Place};
use crate::bridge::html::{
    arg_node, edit, finish_insert, guard_doc_child_order, owning_doc, prepare_insert,
    splice_or_insert, wrap_html_node, HtmlSelf, Insert,
};
use crate::bridge::string::{ruby_verified_data, ruby_verified_text};
use crate::bridge::wrapper::invalidate_indexes;
use crate::lexbor::adapter::html::{Insertion, RawNode, TYPE_ATTRIBUTE, TYPE_ELEMENT};

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// `node.add_child(child)` -> child.
pub fn add_child(_ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let parent = edit(&this)?;
    guard_doc_child_order(Insertion::append(parent.node(), arg_node(&rb_child)?))?;
    let (ins, adopt_from) = prepare_insert(parent, rb_child)?;
    splice_or_insert(parent, ins, Insert::Child, false);
    invalidate_indexes(this.document);
    finish_insert(rb_self, rb_child, ins, adopt_from)
}

/// `node << child` -> node (chainable).
pub fn lshift(ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    add_child(ruby, this, rb_child)?;
    Ok(rb_self)
}

/// `node.add_previous_sibling(node)` / `before` -> node.
pub fn before(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let reference = edit(&this)?;
    let Some(parent) = reference.parent() else {
        return Err(makiri_error(
            "cannot add a sibling to a node with no parent",
        ));
    };
    guard_doc_child_order(Insertion::before(
        parent.node(),
        Some(reference.node()),
        arg_node(&rb_node)?,
    ))?;
    let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
    splice_or_insert(reference, ins, Insert::Before, false);
    invalidate_indexes(this.document);
    finish_insert(rb_self, rb_node, ins, adopt_from)
}

/// `node.add_next_sibling(node)` / `after` -> node.
pub fn after(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let reference = edit(&this)?;
    let Some(parent) = reference.parent() else {
        return Err(makiri_error(
            "cannot add a sibling to a node with no parent",
        ));
    };
    guard_doc_child_order(Insertion::before(
        parent.node(),
        reference.next().map(|n| n.node()),
        arg_node(&rb_node)?,
    ))?;
    let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
    splice_or_insert(reference, ins, Insert::After, true);
    invalidate_indexes(this.document);
    finish_insert(rb_self, rb_node, ins, adopt_from)
}

/// `node.remove` / `node.unlink` -> node.
pub fn remove(_ruby: &Ruby, this: HtmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    let node = edit(&this)?;
    if node.node().node_type() == TYPE_ATTRIBUTE {
        return Err(makiri_error("use delete(name) to remove an attribute"));
    }
    if node.parent().is_some() {
        node.detach();
        invalidate_indexes(this.document);
    }
    Ok(rb_self)
}

/// `node.replace(other)` -> other.
pub fn replace(_ruby: &Ruby, this: HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let reference = edit(&this)?;
    let Some(parent) = reference.parent() else {
        return Err(makiri_error("cannot replace a node with no parent"));
    };
    guard_doc_child_order(Insertion::replacing(
        parent.node(),
        reference.node(),
        arg_node(&rb_other)?,
    ))?;
    let (ins, adopt_from) = prepare_insert(reference, rb_other)?;
    splice_or_insert(reference, ins, Insert::Before, false);
    reference.detach();
    invalidate_indexes(this.document);
    finish_insert(rb_self, rb_other, ins, adopt_from)
}

/* ------------------------------------------------------------------ *
 * attribute and content mutation                                     *
 * ------------------------------------------------------------------ */

/// `element[name] = value` -> value.
pub fn aset(_ruby: &Ruby, this: HtmlSelf, rb_name: Value, rb_value: Value) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Err(makiri_error(
            "cannot set an attribute on a non-element node",
        ));
    };
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    let vv = ruby_verified_data(rb_value, c"attribute value")?;
    if !crate::bridge::html::set_attribute(el, &nv, &vv) {
        return Err(makiri_error("failed to set attribute"));
    }
    invalidate_indexes(this.document);
    Ok(rb_value)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Err(makiri_error(
            "cannot set an attribute on a non-element node",
        ));
    };
    let qv = ruby_verified_text(rb_qname, c"attribute qualified name")?;
    let vv = ruby_verified_data(rb_value, c"attribute value")?;
    let nv = if rb_ns.is_nil() {
        None
    } else {
        Some(ruby_verified_text(rb_ns, c"namespace")?)
    };
    if !crate::bridge::html::set_attribute_ns(el, nv.as_ref(), &qv, &vv) {
        return Err(makiri_error("failed to set namespaced attribute"));
    }
    invalidate_indexes(this.document);
    Ok(rb_value)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Ok(ruby.qnil().as_value());
    };
    let lv = ruby_verified_text(rb_local, c"attribute local name")?;
    let nv = if rb_ns.is_nil() {
        None
    } else {
        Some(ruby_verified_text(rb_ns, c"namespace")?)
    };
    if crate::bridge::html::remove_attribute_ns(el, nv.as_ref(), &lv) {
        invalidate_indexes(this.document);
    }
    Ok(ruby.qnil().as_value())
}

/// `element.name = new_name` -> new_name.
pub fn set_name(_ruby: &Ruby, this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Err(makiri_error("name= is only supported on elements"));
    };
    let nv = ruby_verified_text(rb_name, c"element name")?;
    if !crate::bridge::html::rename(el, &nv) {
        return Err(makiri_error("failed to rename element"));
    }
    invalidate_indexes(this.document);
    Ok(rb_name)
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, this: HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    let node = edit(&this)?;
    let tv = ruby_verified_data(rb_text, c"node content")?;
    if !crate::bridge::html::set_text_content(node, &tv) {
        return Err(makiri_error("failed to set node content"));
    }
    invalidate_indexes(this.document);
    Ok(rb_text)
}

/// `element.delete(name)` -> self.
pub fn delete(_ruby: &Ruby, this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let Some(el) = edit(&this)?.element_mut() else {
        return Ok(rb_self);
    };
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    crate::bridge::html::remove_attribute(el, &nv);
    invalidate_indexes(this.document);
    Ok(rb_self)
}

/// `element.inner_html = html` -> html.
pub fn set_inner_html(_ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    let node = edit(&this)?;
    if node.node().node_type() != TYPE_ELEMENT {
        return Err(makiri_error("inner_html= requires an element"));
    }
    let frag = parse_fragment_in(node.node(), rb_html)?;

    /* Only now that the input parsed: detach the existing children (the arena
     * reclaims them at document destroy) and put the new ones in. */
    while let Some(c) = node.first_child() {
        c.detach();
    }
    splice_fragment(frag, node, Place::Append)?;
    invalidate_indexes(this.document);
    Ok(rb_html)
}

/// `node.outer_html = html` -> html.
pub fn set_outer_html(_ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    let node = edit(&this)?;
    let parent = node.parent();
    if parent.is_none_or(|p| p.node().node_type() != TYPE_ELEMENT) {
        return Err(makiri_error(
            "outer_html= requires a node with a parent element",
        ));
    }
    let parent = parent.expect("checked just above");
    let frag = parse_fragment_in(parent.node(), rb_html)?;
    splice_fragment(frag, node, Place::Before)?;
    node.detach();
    invalidate_indexes(this.document);
    Ok(rb_html)
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
pub fn create_element(_ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let nv = ruby_verified_text(rb_name, c"element name")?;
    created(
        crate::bridge::html::create_element(doc, &nv),
        rb_self,
        "element",
    )
}

/// `Document#create_text_node(content)` -> Text.
pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"text content")?;
    created(
        crate::bridge::html::create_text(doc, &tv),
        rb_self,
        "text node",
    )
}

/// `Document#create_comment(content)` -> Comment.
pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"comment content")?;
    created(
        crate::bridge::html::create_comment(doc, &tv),
        rb_self,
        "comment",
    )
}

/// `Document#create_processing_instruction(target, data)` -> PI.
pub fn create_pi(
    _ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_text(rb_target, c"processing instruction target")?;
    let dv = ruby_verified_text(rb_data, c"processing instruction data")?;
    created(
        crate::bridge::html::create_pi(doc, &tv, &dv),
        rb_self,
        "processing instruction",
    )
}

/// `Document#create_document_type(name, public_id = "", system_id = "")`.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let args =
        magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
            args,
        )?;
    let (rb_name,) = args.required;
    let (rb_pub, rb_sys_) = args.optional;

    let doc = owning_doc(&rb_self)?;
    let nv = ruby_verified_text(rb_name, c"doctype name")?;
    if !crate::bridge::html::valid_doctype_name(&nv) {
        /* The caller's error, not Lexbor's, so the exception class is picked
         * here - the check itself is the DOM layer's. */
        return Err(Error::new(
            ruby.exception_arg_error(),
            "invalid doctype name",
        ));
    }

    let verified =
        |v: Option<Value>, what: &'static core::ffi::CStr| match v.filter(|v| !v.is_nil()) {
            Some(v) => ruby_verified_text(v, what).map(Some),
            None => Ok(None),
        };
    let pv = verified(rb_pub, c"doctype public id")?;
    let sv = verified(rb_sys_, c"doctype system id")?;
    created(
        crate::bridge::html::create_doctype(doc, &nv, pv.as_ref(), sv.as_ref()),
        rb_self,
        "doctype",
    )
}

/// `Document#create_document_fragment` -> an EMPTY DocumentFragment.
pub fn create_document_fragment(_ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    created(
        doc.create_fragment().map(RawNode::from),
        rb_self,
        "document fragment",
    )
}
