//! The XML node's mutators and the Document factories.
//!
//! The Ruby surface of each edit: the arguments converted and checked, the
//! error each refusal raises, the value handed back. The arena itself is only
//! ever reached through [`Editing::with_arena`] or [`with_arena_for_new_node`],
//! after every argument is converted -
//! converting runs Ruby, and the closure it lends the arena to runs none - and
//! what holds two arenas at once (adopting, importing) is a primitive in
//! [`crate::bridge::xml`]. So this module holds no unsafe.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, RArray, RHash, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::bridge::string::namespace_arg;

use crate::bridge::xml::{
    begin_edit, import_copy, incoming_node, verified_text, verified_text_opt,
    with_arena_for_new_node, wrap_xml_node as wrap, xml_mut_result, Editing, XmlSelf,
};
use crate::init::CLASS_XML_DOCUMENT;
use crate::xml::dom_name::split_loose_dom_name;
use crate::xml::model::{ArenaKind, NodeId};
use crate::xml::mutate::{self, place, Place};

/* ------------------------------------------------------------------ */
/* in-place edits                                                     */
/* ------------------------------------------------------------------ */

/// `#remove` / `#unlink` -> self.
pub fn remove(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let rb_self = this.value;
        if crate::bridge::ruby::is_kind_of(rb_self, &CLASS_XML_DOCUMENT) {
            return Err(makiri_error("cannot remove the document node"));
        }
        let edit = begin_edit(this)?;
        edit.with_arena(mutate::remove)?;
        Ok(rb_self)
    })
}

/// Whether `n`, in the receiver's document, is an element.
fn is_element(this: &XmlSelf, n: NodeId) -> bool {
    this.doc_ref().type_(n) == Some(ArenaKind::Element)
}

/// The receiver cleared for an edit, which must be an element.
fn element_for(this: XmlSelf) -> Result<Editing, Error> {
    let edit = begin_edit(this)?;
    if !is_element(&this, edit.id()) {
        return Err(makiri_error(
            "cannot set an attribute on a non-element node",
        ));
    }
    Ok(edit)
}

/// `element[name] = value` -> value.
pub fn aset(_ruby: &Ruby, this: XmlSelf, name: Value, val: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = element_for(this)?;
        let nv = verified_text(name, "attribute name")?;
        let vv = verified_text(val, "attribute value")?;
        let (name, value) = (nv.as_bytes(), vv.as_bytes());
        xml_mut_result(edit.with_arena(|d, n| mutate::set_attribute(d, n, name, value))?)?;
        Ok(val)
    })
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    qname: Value,
    val: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = element_for(this)?;
        let qv = verified_text(qname, "attribute qualified name")?;
        let vv = verified_text(val, "attribute value")?;
        let nv = namespace_arg(ns, "namespace")?;
        let (ns, qname, value) = (
            nv.as_ref().map_or(&b""[..], |n| n.as_bytes()),
            qv.as_bytes(),
            vv.as_bytes(),
        );
        xml_mut_result(edit.with_arena(|d, n| mutate::set_attribute_ns(d, n, ns, qname, value))?)?;
        Ok(val)
    })
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> self.
pub fn remove_attribute_ns(
    _ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let rb_self = this.value;
        let edit = begin_edit(this)?;
        if !is_element(&this, edit.id()) {
            return Ok(rb_self);
        }
        let lv = verified_text(local, "attribute local name")?;
        let nv = namespace_arg(ns, "namespace")?;
        let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
        let local = lv.as_bytes();
        edit.with_arena(|d, n| mutate::remove_attribute_ns(d, n, ns, local))?;
        Ok(rb_self)
    })
}

/// `element.delete(name)` / `#remove_attribute` -> self.
pub fn delete(_ruby: &Ruby, this: XmlSelf, name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let rb_self = this.value;
        let edit = begin_edit(this)?;
        if !is_element(&this, edit.id()) {
            return Ok(rb_self);
        }
        let nv = verified_text(name, "attribute name")?;
        let name = nv.as_bytes();
        edit.with_arena(|d, n| mutate::remove_attribute(d, n, name))?;
        Ok(rb_self)
    })
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, this: XmlSelf, text: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let edit = begin_edit(this)?;
        let tv = verified_text(text, "node content")?;
        let bytes = tv.as_bytes();
        xml_mut_result(edit.with_arena(|d, n| mutate::set_content(d, n, bytes))?)?;
        Ok(text)
    })
}

/* ------------------------------------------------------------------ */
/* building: insertion                                                */
/* ------------------------------------------------------------------ */

/// Put `arg` at `at` relative to the receiver - moved when it is of this
/// document, adopted from its own otherwise - and return it.
fn insert(this: XmlSelf, arg: Value, at: Place) -> Result<Value, Error> {
    let edit = begin_edit(this)?;
    let (node, adoption) = incoming_node(edit.document(), arg)?;
    xml_mut_result(edit.with_arena(|d, target| place(d, target, node, at))?)?;
    if let Some(a) = adoption {
        a.finish();
    }
    Ok(wrap(node, this.document))
}

pub fn add_child(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(this, arg, Place::Child))
}
pub fn before(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(this, arg, Place::Before))
}
pub fn after(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(this, arg, Place::After))
}
pub fn replace(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| insert(this, arg, Place::Replace))
}

/// `element << node` -> self.
pub fn lshift(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let rb_self = this.value;
        insert(this, arg, Place::Child)?;
        Ok(rb_self)
    })
}

/// `clone_node(deep = false)` -> a detached copy in the same document.
///
/// The ONE method here that takes a node receiver and still uses the factory
/// gate rather than [`Editing`]: it reads the receiver and builds something
/// DETACHED, so there is nothing to invalidate and nothing on the receiver to
/// change - freezing a node does not stop it being copied. Every other method
/// taking `XmlSelf` edits the tree and goes through `begin_edit`.
///
/// A Document is refused: its copy would be a second DOCUMENT node in this
/// same arena, which wraps back to the receiver - so the caller got the
/// original under the name of a copy (and `#clone(freeze: true)` froze it),
/// with a stray node left behind. `XML::Document#dup` is the document copy.
pub fn clone_node(this: XmlSelf, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
        let deep = a.optional.0.is_some_and(|v| v.to_bool());
        if crate::bridge::ruby::same_value(this.value, this.document) {
            return Err(makiri_error("clone_node cannot copy a document; use #dup"));
        }
        let copy = xml_mut_result(with_arena_for_new_node(this.document, |d| {
            mutate::clone_node(d, this.id, deep)
        })?)?;
        Ok(wrap(copy, this.document))
    })
}

/* ------------------------------------------------------------------ */
/* document factories                                                 */
/* ------------------------------------------------------------------ */

/// `create_element(name, content = nil, attributes = {})` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(Value,), (), RArray, (), (), ()>(args)?;
        let (name,) = a.required;
        let mut content = ruby.qnil().as_value();
        let mut attrs: Option<RHash> = None;
        for v in a.splat.into_iter() {
            if let Some(h) = RHash::from_value(v) {
                attrs = Some(h);
            } else if !v.is_nil() {
                content = v;
            }
        }

        let nv = verified_text(name, "element name")?;
        let cv = verified_text_opt(content, "element content")?;
        let name = nv.as_bytes();
        let el = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
            mutate::new_element(d, name)
        })?)?;
        if let Some(cv) = &cv {
            let text = cv.as_bytes();
            xml_mut_result(with_arena_for_new_node(rb_self, |d| {
                mutate::set_content(d, el, text)
            })?)?;
        }
        /* Both views are done with. Drop them before any Ruby runs again (the
         * wrapper allocation, the attribute loop's `to_s`), so no borrow of a
         * Ruby String is held across a GC point. */
        drop((nv, cv));
        let rb_el = wrap(el, rb_self);
        if let Some(h) = attrs {
            /* Keys and values are stringified - Nokogiri accepts symbol keys and
             * non-string values - then go through the normal validated setter,
             * after the pairs are out of the Hash (`kwargs::each_pair`). */
            /* `rb_el` was wrapped just above, so it converts. */
            let el_self = <XmlSelf as magnus::TryConvert>::try_convert(rb_el)?;
            crate::glue::kwargs::each_pair(ruby, h, |k, v| {
                let (k, v) = (crate::bridge::ruby::to_s(k)?, crate::bridge::ruby::to_s(v)?);
                aset(ruby, el_self, k.as_value(), v.as_value())?;
                Ok(())
            })?;
        }
        Ok(rb_el)
    })
}

/// `create_loose_dom_element(qualified_name, prefix, local_name, namespace_uri)`
/// -> Element.
pub fn create_loose_dom_element(
    ruby: &Ruby,
    rb_self: Value,
    qname: Value,
    prefix: Value,
    local: Value,
    ns: Value,
) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let qv = verified_text(qname, "qualified name")?;
        let lv = verified_text(local, "local name")?;
        let pv = verified_text_opt(prefix, "prefix")?;
        let nv = namespace_arg(ns, "namespace URI")?;

        let qname = qv.as_bytes();
        let sp = split_loose_dom_name(qname, pv.as_ref().map(|p| p.as_bytes()), lv.as_bytes())
            .map_err(|e| Error::new(ruby.exception_arg_error(), e.message()))?;
        let ns = nv.as_ref().map_or(&b""[..], |n| n.as_bytes());
        let el = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
            mutate::new_loose_dom_element(d, qname, sp, ns)
        })?)?;
        Ok(wrap(el, rb_self))
    })
}

/// `create_document_type(name, public_id = "", system_id = "")` -> DocumentType.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<
            (Value,),
            (Option<Value>, Option<Value>),
            (),
            (),
            (),
            (),
        >(args)?;
        let name = a.required.0;
        let nil = ruby.qnil().as_value();
        let nv = verified_text(name, "doctype name")?;
        let pv = verified_text_opt(a.optional.0.unwrap_or(nil), "doctype public id")?;
        let sv = verified_text_opt(a.optional.1.unwrap_or(nil), "doctype system id")?;
        /* An empty id is absent, like nil, matching the HTML factory and Nokogiri. */
        fn id(v: &Option<crate::bridge::string::RubyText>) -> Option<&[u8]> {
            v.as_ref().map(|v| v.as_bytes()).filter(|b| !b.is_empty())
        }
        let (name, pub_id, sys_id) = (nv.as_bytes(), id(&pv), id(&sv));
        let dt = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
            mutate::new_document_type(d, name, pub_id, sys_id)
        })?)?;
        Ok(wrap(dt, rb_self))
    })
}

/// The shared body of the leaf-data factories.
fn create_chardata(
    rb_self: Value,
    text: Value,
    type_: ArenaKind,
    what: &str,
) -> Result<Value, Error> {
    let tv = verified_text(text, what)?;
    let bytes = tv.as_bytes();
    let n = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
        mutate::new_chardata(d, type_, bytes)
    })?)?;
    Ok(wrap(n, rb_self))
}

pub fn create_text_node(_ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| create_chardata(rb_self, t, ArenaKind::Text, "text content"))
}
pub fn create_comment(_ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        create_chardata(rb_self, t, ArenaKind::Comment, "comment content")
    })
}
pub fn create_cdata(_ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        create_chardata(rb_self, t, ArenaKind::CDataSection, "CDATA content")
    })
}

pub fn create_pi(_ruby: &Ruby, rb_self: Value, target: Value, data: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let tg = verified_text(target, "PI target")?;
        let dt = verified_text(data, "PI data")?;
        let (target, data) = (tg.as_bytes(), dt.as_bytes());
        let pi = xml_mut_result(with_arena_for_new_node(rb_self, |d| {
            mutate::new_pi(d, target, data)
        })?)?;
        Ok(wrap(pi, rb_self))
    })
}

/// `Document#import_node(node, deep = false)` - the DOM's importNode.
pub fn import_node(_ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
        let deep = a.optional.0.is_some_and(|v| v.to_bool());
        Ok(wrap(import_copy(rb_self, a.required.0, deep)?, rb_self))
    })
}
