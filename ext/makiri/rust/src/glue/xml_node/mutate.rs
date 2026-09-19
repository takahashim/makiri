//! The XML node's mutators and the Document factories (glue/ruby_xml_node.c).
//!
//! The Ruby surface of each edit: the arguments converted and checked, the
//! error each refusal raises, the value handed back. The arena itself is only
//! ever reached through [`with_arena_mut`], after every argument is converted -
//! converting runs Ruby, and the closure it lends the arena to runs none - and
//! what holds two arenas at once (adopting, importing) is a primitive in
//! [`crate::bridge::xml`]. So this module holds no unsafe.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, RArray, RHash, Ruby, Value};

use crate::bridge::ruby::error_class;
use crate::bridge::xml::{
    begin_edit, import_copy, incoming_node, verified_text, verified_text_opt, with_arena_mut, wrap,
    xml_mut_check, xml_wrap_rel_value, XmlSelf,
};
use crate::init::CLASS_XML_DOCUMENT;
use crate::xml::api::{
    xml_clone_node, xml_new_chardata, xml_new_document_type, xml_new_element,
    xml_new_loose_dom_element, xml_new_pi, xml_remove, xml_remove_attribute,
    xml_remove_attribute_ns, xml_rename, xml_set_attribute, xml_set_attribute_ns, xml_set_content,
};
use crate::xml::model::{NodeId, NodeType};
use crate::xml::mutate::{place, Place};
use crate::xml::qname::split_loose_dom_name;

/* ------------------------------------------------------------------ */
/* in-place edits                                                     */
/* ------------------------------------------------------------------ */

/// `#remove` / `#unlink` -> self.
pub fn remove(this: XmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    if rb_self.is_kind_of(CLASS_XML_DOCUMENT.class()) {
        return Err(Error::new(error_class(), "cannot remove the document node"));
    }
    let n = begin_edit(this)?;
    with_arena_mut(this.document, |d| xml_remove(d, n))?;
    Ok(rb_self)
}

/// Whether `n`, in the receiver's document, is an element.
fn is_element(this: &XmlSelf, n: NodeId) -> bool {
    this.doc_ref().type_(n) == Some(NodeType::Element)
}

/// The receiver cleared for an edit, which must be an element.
fn element_for(this: XmlSelf) -> Result<NodeId, Error> {
    let n = begin_edit(this)?;
    if !is_element(&this, n) {
        return Err(Error::new(
            error_class(),
            "cannot set an attribute on a non-element node",
        ));
    }
    Ok(n)
}

/// `element[name] = value` -> value.
pub fn aset(_ruby: &Ruby, this: XmlSelf, name: Value, val: Value) -> Result<Value, Error> {
    let n = element_for(this)?;
    let nv = verified_text(name, c"attribute name")?;
    let vv = verified_text(val, c"attribute value")?;
    let (name, value) = (nv.as_verified().as_bytes(), vv.as_verified().as_bytes());
    let mut out = NodeId::INVALID;
    let st = with_arena_mut(this.document, |d| {
        xml_set_attribute(d, n, name, value, &mut out)
    })?;
    xml_mut_check(st)?;
    Ok(val)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    qname: Value,
    val: Value,
) -> Result<Value, Error> {
    let n = element_for(this)?;
    let qv = verified_text(qname, c"attribute qualified name")?;
    let vv = verified_text(val, c"attribute value")?;
    let nv = verified_text_opt(ns, c"namespace")?;
    let (ns, qname, value) = (
        nv.as_verified().as_bytes(),
        qv.as_verified().as_bytes(),
        vv.as_verified().as_bytes(),
    );
    let mut out = NodeId::INVALID;
    let st = with_arena_mut(this.document, |d| {
        xml_set_attribute_ns(d, n, ns, qname, value, &mut out)
    })?;
    xml_mut_check(st)?;
    Ok(val)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> self.
pub fn remove_attribute_ns(
    _ruby: &Ruby,
    this: XmlSelf,
    ns: Value,
    local: Value,
) -> Result<Value, Error> {
    let rb_self = this.value;
    let n = begin_edit(this)?;
    if !is_element(&this, n) {
        return Ok(rb_self);
    }
    let lv = verified_text(local, c"attribute local name")?;
    let nv = verified_text_opt(ns, c"namespace")?;
    let (ns, local) = (nv.as_verified().as_bytes(), lv.as_verified().as_bytes());
    with_arena_mut(this.document, |d| xml_remove_attribute_ns(d, n, ns, local))?;
    Ok(rb_self)
}

/// `element.delete(name)` / `#remove_attribute` -> self.
pub fn delete(_ruby: &Ruby, this: XmlSelf, name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let n = begin_edit(this)?;
    if !is_element(&this, n) {
        return Ok(rb_self);
    }
    let nv = verified_text(name, c"attribute name")?;
    let name = nv.as_verified().as_bytes();
    with_arena_mut(this.document, |d| xml_remove_attribute(d, n, name))?;
    Ok(rb_self)
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, this: XmlSelf, text: Value) -> Result<Value, Error> {
    let n = begin_edit(this)?;
    let tv = verified_text(text, c"node content")?;
    let bytes = tv.as_verified().as_bytes();
    xml_mut_check(with_arena_mut(this.document, |d| {
        xml_set_content(d, n, bytes)
    })?)?;
    Ok(text)
}

/// `node.name = new_name` -> new_name.
pub fn set_name(_ruby: &Ruby, this: XmlSelf, name: Value) -> Result<Value, Error> {
    let n = begin_edit(this)?;
    let nv = verified_text(name, c"node name")?;
    let bytes = nv.as_verified().as_bytes();
    xml_mut_check(with_arena_mut(this.document, |d| xml_rename(d, n, bytes))?)?;
    Ok(name)
}

/* ------------------------------------------------------------------ */
/* building: insertion                                                */
/* ------------------------------------------------------------------ */

/// Put `arg` at `at` relative to the receiver - moved when it is of this
/// document, adopted from its own otherwise - and return it.
fn insert(this: XmlSelf, arg: Value, at: Place) -> Result<Value, Error> {
    let target = begin_edit(this)?;
    let (node, adoption) = incoming_node(this.document, arg)?;
    xml_mut_check(with_arena_mut(this.document, |d| {
        place(d, target, node, at)
    })?)?;
    if let Some(a) = adoption {
        a.finish();
    }
    Ok(wrap(node, this.document))
}

pub fn add_child(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(this, arg, Place::Child)
}
pub fn before(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(this, arg, Place::Before)
}
pub fn after(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(this, arg, Place::After)
}
pub fn replace(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    insert(this, arg, Place::Replace)
}

/// `element << node` -> self.
pub fn lshift(_ruby: &Ruby, this: XmlSelf, arg: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    insert(this, arg, Place::Child)?;
    Ok(rb_self)
}

/// `clone_node(deep = false)` -> a detached copy in the same document.
pub fn clone_node(this: XmlSelf, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(), (Option<Value>,), (), (), (), ()>(args)?;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());
    let mut out = NodeId::INVALID;
    xml_mut_check(with_arena_mut(this.document, |d| {
        xml_clone_node(d, this.id, deep, &mut out)
    })?)?;
    Ok(xml_wrap_rel_value(this, out))
}

/* ------------------------------------------------------------------ */
/* document factories                                                 */
/* ------------------------------------------------------------------ */

/// `create_element(name, content = nil, attributes = {})` -> Element.
pub fn create_element(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
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

    let nv = verified_text(name, c"element name")?;
    let cv = verified_text_opt(content, c"element content")?;
    let (name, text) = (nv.as_verified().as_bytes(), cv.as_verified().as_bytes());
    let mut el = NodeId::INVALID;
    xml_mut_check(with_arena_mut(rb_self, |d| {
        xml_new_element(d, name, &mut el)
    })?)?;
    if !content.is_nil() {
        xml_mut_check(with_arena_mut(rb_self, |d| xml_set_content(d, el, text))?)?;
    }
    let rb_el = wrap(el, rb_self);
    if let Some(h) = attrs {
        /* Keys and values are stringified - Nokogiri accepts symbol keys and
         * non-string values - then go through the normal validated setter. */
        let pairs: RArray = h.funcall("to_a", ())?;
        for pair in pairs.into_iter() {
            let entry = RArray::from_value(pair).expect("Hash#to_a yields pairs");
            let k: Value = entry.entry(0)?;
            let v: Value = entry.entry(1)?;
            /* `rb_el` was wrapped just above, so it converts. */
            let el_self = <XmlSelf as magnus::TryConvert>::try_convert(rb_el)?;
            aset(
                ruby,
                el_self,
                k.funcall("to_s", ())?,
                v.funcall("to_s", ())?,
            )?;
        }
    }
    Ok(rb_el)
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
    let qv = verified_text(qname, c"qualified name")?;
    let lv = verified_text(local, c"local name")?;
    let pv = (!prefix.is_nil())
        .then(|| verified_text(prefix, c"prefix"))
        .transpose()?;
    let nv = verified_text_opt(ns, c"namespace URI")?;

    let qname = qv.as_verified().as_bytes();
    let sp = split_loose_dom_name(
        qname,
        pv.as_ref().map(|p| p.as_verified().as_bytes()),
        lv.as_verified().as_bytes(),
    )
    .map_err(|e| Error::new(ruby.exception_arg_error(), e.message()))?;
    let ns = nv.as_verified().as_bytes();
    let mut el = NodeId::INVALID;
    xml_mut_check(with_arena_mut(rb_self, |d| {
        xml_new_loose_dom_element(d, qname, sp, ns, &mut el)
    })?)?;
    Ok(wrap(el, rb_self))
}

/// `create_document_type(name, public_id = "", system_id = "")` -> DocumentType.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
        args,
    )?;
    let name = a.required.0;
    let nil = ruby.qnil().as_value();
    let nv = verified_text(name, c"doctype name")?;
    let pv = verified_text_opt(a.optional.0.unwrap_or(nil), c"doctype public id")?;
    let sv = verified_text_opt(a.optional.1.unwrap_or(nil), c"doctype system id")?;
    /* An empty id is absent (NULL), matching the HTML factory and Nokogiri. */
    let (name, pub_id, sys_id) = (
        nv.as_verified().as_bytes(),
        (pv.len() != 0).then(|| pv.as_verified().as_bytes()),
        (sv.len() != 0).then(|| sv.as_verified().as_bytes()),
    );
    let mut dt = NodeId::INVALID;
    xml_mut_check(with_arena_mut(rb_self, |d| {
        xml_new_document_type(d, name, pub_id, sys_id, &mut dt)
    })?)?;
    Ok(wrap(dt, rb_self))
}

/// The shared body of the leaf-data factories.
fn create_chardata(
    rb_self: Value,
    text: Value,
    type_: NodeType,
    what: &core::ffi::CStr,
) -> Result<Value, Error> {
    let tv = verified_text(text, what)?;
    let bytes = tv.as_verified().as_bytes();
    let mut n = NodeId::INVALID;
    xml_mut_check(with_arena_mut(rb_self, |d| {
        xml_new_chardata(d, type_, bytes, &mut n)
    })?)?;
    Ok(wrap(n, rb_self))
}

pub fn create_text_node(_ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(rb_self, t, NodeType::Text, c"text content")
}
pub fn create_comment(_ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(rb_self, t, NodeType::Comment, c"comment content")
}
pub fn create_cdata(_ruby: &Ruby, rb_self: Value, t: Value) -> Result<Value, Error> {
    create_chardata(rb_self, t, NodeType::CData, c"CDATA content")
}

pub fn create_pi(_ruby: &Ruby, rb_self: Value, target: Value, data: Value) -> Result<Value, Error> {
    let tg = verified_text(target, c"PI target")?;
    let dt = verified_text(data, c"PI data")?;
    let (target, data) = (tg.as_verified().as_bytes(), dt.as_verified().as_bytes());
    let mut pi = NodeId::INVALID;
    xml_mut_check(with_arena_mut(rb_self, |d| {
        xml_new_pi(d, target, data, &mut pi)
    })?)?;
    Ok(wrap(pi, rb_self))
}

/// `Document#import_node(node, deep = false)` - the DOM's importNode.
pub fn import_node(_ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let deep = a.optional.0.is_some_and(|v| v.to_bool());
    Ok(wrap(import_copy(rb_self, a.required.0, deep)?, rb_self))
}
