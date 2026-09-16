//! The XML node's readers: name, namespace, DTD identifiers, content,
//! navigation and attributes (glue/ruby_xml_node.c, first third).
//!
//! A node is an index-arena `NodeId`, so every reader resolves through its
//! Document. XML nodes never inherit the Lexbor HTML readers - those live on
//! `Makiri::HTML::NodeMethods` - so this surface is structural.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, Ruby, Value};

use super::abi::*;

/// Wrap an optional reached node under `rb_self`'s Document (invalid -> nil).
fn wrap_rel(this: super::XmlSelf, rel: Option<NodeId>) -> Value {
    super::wrap(rel.unwrap_or(NodeId::INVALID), this.document)
}

/* ---- name ---- */

pub fn name(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        match d.type_(id) {
            Some(NodeType::Element | NodeType::Attribute) => str_span(ruby, d, d.node(id).qname),
            Some(NodeType::Pi | NodeType::Doctype) => str_span(ruby, d, d.node(id).local),
            Some(NodeType::Text) => ruby.str_new("text").as_value(),
            Some(NodeType::CData) => ruby.str_new("#cdata-section").as_value(),
            Some(NodeType::Comment) => ruby.str_new("comment").as_value(),
            Some(NodeType::Fragment) => ruby.str_new("#document-fragment").as_value(),
            _ => ruby.str_new("document").as_value(),
        }
    }
}

pub fn local_name(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        if d.type_(id) == Some(NodeType::Element) || d.type_(id) == Some(NodeType::Attribute) {
            return str_span(ruby, d, d.node(id).local);
        }
        ruby.qnil().as_value()
    }
}

/// `#prefix`. A zero-length prefix means unprefixed, which is nil rather than
/// `""` - the distinction `#namespace` depends on.
pub fn prefix(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        if d.node(id).prefix.len == 0 {
            return ruby.qnil().as_value();
        }
        str_span(ruby, d, d.node(id).prefix)
    }
}

pub fn namespace_uri(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        if d.node(id).ns_uri.len == 0 {
            return ruby.qnil().as_value();
        }
        str_span(ruby, d, d.node(id).ns_uri)
    }
}

pub fn node_type(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        let ty = d.type_(this.id).map_or(0, |t| t.as_u32());
        ruby.integer_from_i64(ty as i64).as_value()
    }
}

/* ---- DTD identifiers ----
 *
 * The doctype node repurposes fields: local/qname are the DOCTYPE name
 * (`#name`), prefix the PUBLIC id, value the SYSTEM id. An ABSENT field means
 * that id was omitted and answers nil; an empty literal (`PUBLIC ""`) is a
 * present zero-length span and answers `""`. */

pub fn dtd_external_id(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        str_span_or_nil(ruby, d, d.node(this.id).prefix)
    }
}

pub fn dtd_system_id(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        str_span_or_nil(ruby, d, d.node(this.id).value)
    }
}

/* ---- content ---- */

pub fn content(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        let id = this.id;
        if matches!(
            d.type_(id),
            Some(
                NodeType::Text
                    | NodeType::CData
                    | NodeType::Comment
                    | NodeType::Attribute
                    | NodeType::Pi
            )
        ) {
            return str_span(ruby, d, d.node(id).value);
        }

        let mut out: Vec<u8> = Vec::new();
        let mut cur = d.first_child(id);
        while let Some(c) = cur {
            if matches!(d.type_(c), Some(NodeType::Text | NodeType::CData)) {
                out.extend_from_slice(d.value(c));
            }
            if d.first_child(c).is_some() {
                cur = d.first_child(c);
                continue;
            }
            while let Some(x) = cur {
                if x != id && d.next(x).is_none() {
                    cur = d.parent(x);
                } else {
                    break;
                }
            }
            match cur {
                None => break,
                Some(x) if x == id => break,
                Some(_) => cur = d.next(cur.unwrap()),
            }
        }
        utf8(ruby, &out).as_value()
    }
}

pub fn value(ruby: &Ruby, this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        str_span(ruby, d, d.node(this.id).value)
    }
}

/* ---- navigation ---- */

pub fn parent(this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        wrap_rel(this, d.parent(this.id))
    }
}
pub fn next(this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        wrap_rel(this, d.next(this.id))
    }
}
pub fn previous(this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        wrap_rel(this, d.prev(this.id))
    }
}
pub fn first_child(this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        wrap_rel(this, d.first_child(this.id))
    }
}
pub fn last_child(this: super::XmlSelf) -> Value {
    unsafe {
        let d = &*this.doc();
        wrap_rel(this, d.last_child(this.id))
    }
}

pub fn get_document(this: super::XmlSelf) -> Value {
    this.document
}

/// `#element_children` - the child ELEMENT nodes only, in document order.
pub fn element_children(this: super::XmlSelf) -> Result<Value, Error> {
    unsafe {
        let d = &*this.doc();
        let set = node_set_new(this.document);
        let mut c = d.first_child(this.id);
        while let Some(id) = c {
            if d.type_(id) == Some(NodeType::Element) {
                node_set_push(set.as_raw(), id.to_token() as *mut core::ffi::c_void)?;
            }
            c = d.next(id);
        }
        Ok(set)
    }
}

pub fn children(this: super::XmlSelf) -> Result<Value, Error> {
    unsafe {
        let d = &*this.doc();
        let set = node_set_new(this.document);
        let mut c = d.first_child(this.id);
        while let Some(id) = c {
            node_set_push(set.as_raw(), id.to_token() as *mut core::ffi::c_void)?;
            c = d.next(id);
        }
        Ok(set)
    }
}

/* ---- attributes ---- */

/// The attribute of `el` whose qualified name is exactly `name`.
fn find_attr(d: &XmlDoc, el: NodeId, name: &[u8]) -> Option<NodeId> {
    if d.type_(el) != Some(NodeType::Element) {
        return None;
    }
    let mut a = d.attrs(el);
    while let Some(id) = a {
        if d.qname(id) == name {
            return Some(id);
        }
        a = d.next(id);
    }
    None
}

/// `#[]` - the attribute's value, or nil.
pub fn aref(ruby: &Ruby, this: super::XmlSelf, rb_name: Value) -> Result<Value, Error> {
    let id = this.id;
    // SAFETY: the receiver's arena, borrowed for this statement only.
    if unsafe { (*this.doc()).type_(id) } != Some(NodeType::Element) {
        return Ok(ruby.qnil().as_value());
    }
    /* Convert the name BEFORE borrowing the arena: its `to_str` is Ruby code,
     * and it may edit this same document. */
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    // SAFETY: no Ruby code runs while the arena and the name's bytes are read;
    // building the String only allocates.
    unsafe {
        let d = &*this.doc();
        match find_attr(d, id, nv.bytes()) {
            None => Ok(ruby.qnil().as_value()),
            Some(at) => Ok(str_span(ruby, d, d.node(at).value)),
        }
    }
}

/// The Attr NODE with that qualified name.
pub fn attribute_by_qualified_name(
    ruby: &Ruby,
    this: super::XmlSelf,
    rb_name: Value,
) -> Result<Value, Error> {
    let id = this.id;
    // SAFETY: as in `aref`.
    if unsafe { (*this.doc()).type_(id) } != Some(NodeType::Element) {
        return Ok(ruby.qnil().as_value());
    }
    /* Converted before the arena is borrowed - see `aref`. */
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    // SAFETY: the borrow ends with this statement, before the wrap allocates.
    let a = unsafe { find_attr(&*this.doc(), id, nv.bytes()) };
    Ok(super::wrap(a.unwrap_or(NodeId::INVALID), this.document))
}

pub fn attribute_value_by_qualified_name(
    ruby: &Ruby,
    this: super::XmlSelf,
    rb_name: Value,
) -> Result<Value, Error> {
    aref(ruby, this, rb_name)
}

pub fn attribute_nodes(this: super::XmlSelf) -> Result<Value, Error> {
    unsafe {
        let d = &*this.doc();
        let set = node_set_new(this.document);
        let id = this.id;
        if d.type_(id) == Some(NodeType::Element) {
            let mut a = d.attrs(id);
            while let Some(at) = a {
                node_set_push(set.as_raw(), at.to_token() as *mut core::ffi::c_void)?;
                a = d.next(at);
            }
        }
        Ok(set)
    }
}
