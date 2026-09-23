//! The HTML node's mutators and the Document factories (glue/ruby_html_mutate.c).
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

use crate::bridge::fragment::stage_fragment_in;
use crate::bridge::html::{edit, insert, owning_doc, wrap_html_node, HtmlEdit, HtmlSelf};
use crate::bridge::string::{ruby_verified_data, ruby_verified_text};
use crate::lexbor::adapter::html::{
    HtmlElementMut, Place, RawNode, TYPE_ATTRIBUTE, TYPE_ELEMENT,
};

/// The receiver as an element, once every argument is converted. Its node type
/// was checked before the conversion (an argument cannot change it), so the
/// `None` arm is unreachable - it answers `refusal` rather than assuming so.
fn element_of<'a>(edit: &HtmlEdit<'a>, refusal: &'static str) -> Result<HtmlElementMut<'a>, Error> {
    edit.node()?.element_mut().ok_or_else(|| makiri_error(refusal))
}

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// `node.add_child(child)` -> child.
pub fn add_child(_ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        insert(&this, rb_child, Place::Child)
    })
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
    crate::bridge::ruby::entry(|| {
        insert(&this, rb_node, Place::Before)
    })
}

/// `node.add_next_sibling(node)` / `after` -> node.
pub fn after(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        insert(&this, rb_node, Place::After)
    })
}

/// `node.replace(other)` -> other.
pub fn replace(_ruby: &Ruby, this: HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        insert(&this, rb_other, Place::Replace)
    })
}

/// `node.remove` / `node.unlink` -> node.
pub fn remove(_ruby: &Ruby, this: HtmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let node = edit(&this)?.node()?;
        if node.node().node_type() == TYPE_ATTRIBUTE {
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
pub fn aset(_ruby: &Ruby, this: HtmlSelf, rb_name: Value, rb_value: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "cannot set an attribute on a non-element node";
        let edit = edit(&this)?;
        if edit.node_type() != TYPE_ELEMENT {
            return Err(makiri_error(REFUSAL));
        }
        let nv = ruby_verified_text(rb_name, c"attribute name")?;
        let vv = ruby_verified_data(rb_value, c"attribute value")?;
        let el = element_of(&edit, REFUSAL)?;
        if !crate::bridge::html::set_attribute(el, &nv, &vv) {
            return Err(makiri_error("failed to set attribute"));
        }
        Ok(rb_value)
    })
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "cannot set an attribute on a non-element node";
        let edit = edit(&this)?;
        if edit.node_type() != TYPE_ELEMENT {
            return Err(makiri_error(REFUSAL));
        }
        let qv = ruby_verified_text(rb_qname, c"attribute qualified name")?;
        let vv = ruby_verified_data(rb_value, c"attribute value")?;
        let nv = if rb_ns.is_nil() {
            None
        } else {
            Some(ruby_verified_text(rb_ns, c"namespace")?)
        };
        let el = element_of(&edit, REFUSAL)?;
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
        if edit.node_type() != TYPE_ELEMENT {
            return Ok(ruby.qnil().as_value());
        }
        let lv = ruby_verified_text(rb_local, c"attribute local name")?;
        let nv = if rb_ns.is_nil() {
            None
        } else {
            Some(ruby_verified_text(rb_ns, c"namespace")?)
        };
        let el = element_of(&edit, "remove_attribute_ns requires an element")?;
        crate::bridge::html::remove_attribute_ns(el, nv.as_ref(), &lv);
        Ok(ruby.qnil().as_value())
    })
}

/// `element.name = new_name` -> new_name.
pub fn set_name(_ruby: &Ruby, this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        const REFUSAL: &str = "name= is only supported on elements";
        let edit = edit(&this)?;
        if edit.node_type() != TYPE_ELEMENT {
            return Err(makiri_error(REFUSAL));
        }
        let nv = ruby_verified_text(rb_name, c"element name")?;
        let el = element_of(&edit, REFUSAL)?;
        if !crate::bridge::html::rename(el, &nv) {
            return Err(makiri_error("failed to rename element"));
        }
        Ok(rb_name)
    })
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, this: HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = edit(&this)?;
        let tv = ruby_verified_data(rb_text, c"node content")?;
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
        if edit.node_type() != TYPE_ELEMENT {
            return Ok(rb_self);
        }
        let nv = ruby_verified_text(rb_name, c"attribute name")?;
        let el = element_of(&edit, "delete requires an element")?;
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
        if edit.node_type() != TYPE_ELEMENT {
            return Err(makiri_error("inner_html= requires an element"));
        }
        /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
        let html = string_of(rb_html)?;
        let node = edit.node()?;
        let staged = stage_fragment_in(node, html)?;
        /* Detached, not destroyed: the arena reclaims them with the document. */
        while let Some(c) = node.first_child() {
            c.detach();
        }
        node.place(staged, Place::Child);
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
            .filter(|p| p.node().node_type() == TYPE_ELEMENT)
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
pub fn create_element(_ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let doc = owning_doc(&rb_self)?;
        let nv = ruby_verified_text(rb_name, c"element name")?;
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
        let tv = ruby_verified_data(rb_text, c"text content")?;
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
        let tv = ruby_verified_data(rb_text, c"comment content")?;
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
        let tv = ruby_verified_text(rb_target, c"processing instruction target")?;
        let dv = ruby_verified_text(rb_data, c"processing instruction data")?;
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
