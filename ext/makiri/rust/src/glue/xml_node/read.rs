//! The XML node's readers: name, namespace, DTD identifiers, content,
//! navigation and attributes (glue/ruby_xml_node.c, first third).
//!
//! A node is an index-arena `NodeId`, so every reader resolves through its
//! Document. XML nodes never inherit the Lexbor HTML readers - those live on
//! `Makiri::HTML::NodeMethods` - so this surface is structural.

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, Ruby, Value};

use super::abi::*;
use super::{doc, node_document, unwrap};

/// Wrap an optional reached node under `rb_self`'s Document (invalid -> nil).
unsafe fn wrap_rel(rb_self: Value, rel: Option<NodeId>) -> Value {
    super::wrap(rel.unwrap_or(NodeId::INVALID), node_document(rb_self))
}

/* ---- name ---- */

pub fn name(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        match d.type_(id) {
            T_ELEMENT | T_ATTRIBUTE => str_span(ruby, d, d.node(id).qname),
            T_PI | T_DOCTYPE => str_span(ruby, d, d.node(id).local),
            T_TEXT => ruby.str_new("text").as_value(),
            T_CDATA => ruby.str_new("#cdata-section").as_value(),
            T_COMMENT => ruby.str_new("comment").as_value(),
            T_FRAGMENT => ruby.str_new("#document-fragment").as_value(),
            _ => ruby.str_new("document").as_value(),
        }
    }
}

pub fn local_name(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        if d.type_(id) == T_ELEMENT || d.type_(id) == T_ATTRIBUTE {
            return str_span(ruby, d, d.node(id).local);
        }
        ruby.qnil().as_value()
    }
}

/// `#prefix`. A zero-length prefix means unprefixed, which is nil rather than
/// `""` - the distinction `#namespace` depends on.
pub fn prefix(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        if d.node(id).prefix.len == 0 {
            return ruby.qnil().as_value();
        }
        str_span(ruby, d, d.node(id).prefix)
    }
}

pub fn namespace_uri(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        if d.node(id).ns_uri.len == 0 {
            return ruby.qnil().as_value();
        }
        str_span(ruby, d, d.node(id).ns_uri)
    }
}

pub fn node_type(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        ruby.integer_from_i64(d.type_(unwrap(rb_self)) as i64)
            .as_value()
    }
}

/* ---- DTD identifiers ----
 *
 * The doctype node repurposes fields: local/qname are the DOCTYPE name
 * (`#name`), prefix the PUBLIC id, value the SYSTEM id. An ABSENT field means
 * that id was omitted and answers nil; an empty literal (`PUBLIC ""`) is a
 * present zero-length span and answers `""`. */

pub fn dtd_external_id(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        str_span_or_nil(ruby, d, d.node(unwrap(rb_self)).prefix)
    }
}

pub fn dtd_system_id(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        str_span_or_nil(ruby, d, d.node(unwrap(rb_self)).value)
    }
}

/* ---- content ---- */

pub fn content(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        if matches!(
            d.type_(id),
            T_TEXT | T_CDATA | T_COMMENT | T_ATTRIBUTE | T_PI
        ) {
            return str_span(ruby, d, d.node(id).value);
        }

        let mut out: Vec<u8> = Vec::new();
        let mut cur = d.first_child(id);
        while let Some(c) = cur {
            if matches!(d.type_(c), T_TEXT | T_CDATA) {
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

pub fn value(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        str_span(ruby, d, d.node(unwrap(rb_self)).value)
    }
}

/* ---- navigation ---- */

pub fn parent(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        wrap_rel(rb_self, d.parent(unwrap(rb_self)))
    }
}
pub fn next(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        wrap_rel(rb_self, d.next(unwrap(rb_self)))
    }
}
pub fn previous(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        wrap_rel(rb_self, d.prev(unwrap(rb_self)))
    }
}
pub fn first_child(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        wrap_rel(rb_self, d.first_child(unwrap(rb_self)))
    }
}
pub fn last_child(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        wrap_rel(rb_self, d.last_child(unwrap(rb_self)))
    }
}

pub fn get_document(rb_self: Value) -> Value {
    unsafe { node_document(rb_self) }
}

/// `#element_children` - the child ELEMENT nodes only, in document order.
pub fn element_children(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let set = mkr_node_set_new(node_document(rb_self).as_raw());
        let mut c = d.first_child(unwrap(rb_self));
        while let Some(id) = c {
            if d.type_(id) == T_ELEMENT {
                mkr_node_set_push(set, id.to_token() as *mut core::ffi::c_void);
            }
            c = d.next(id);
        }
        Value::from_raw(set)
    }
}

pub fn children(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let set = mkr_node_set_new(node_document(rb_self).as_raw());
        let mut c = d.first_child(unwrap(rb_self));
        while let Some(id) = c {
            mkr_node_set_push(set, id.to_token() as *mut core::ffi::c_void);
            c = d.next(id);
        }
        Value::from_raw(set)
    }
}

/* ---- attributes ---- */

/// The attribute of `el` whose qualified name is exactly `name`.
unsafe fn find_attr(d: &XmlDoc, el: NodeId, name: &[u8]) -> Option<NodeId> {
    if d.type_(el) != T_ELEMENT {
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
pub fn aref(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        if d.type_(id) != T_ELEMENT {
            return Ok(ruby.qnil().as_value());
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let a = find_attr(d, id, nv.bytes());
        core::hint::black_box(rb_name);
        match a {
            None => Ok(ruby.qnil().as_value()),
            Some(at) => Ok(str_span(ruby, d, d.node(at).value)),
        }
    }
}

/// The Attr NODE with that qualified name.
pub fn attribute_by_qualified_name(
    ruby: &Ruby,
    rb_self: Value,
    rb_name: Value,
) -> Result<Value, Error> {
    unsafe {
        let d = &*doc(rb_self);
        let id = unwrap(rb_self);
        if d.type_(id) != T_ELEMENT {
            return Ok(ruby.qnil().as_value());
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let a = find_attr(d, id, nv.bytes());
        core::hint::black_box(rb_name);
        Ok(super::wrap(
            a.unwrap_or(NodeId::INVALID),
            node_document(rb_self),
        ))
    }
}

pub fn attribute_value_by_qualified_name(
    ruby: &Ruby,
    rb_self: Value,
    rb_name: Value,
) -> Result<Value, Error> {
    aref(ruby, rb_self, rb_name)
}

pub fn attribute_nodes(rb_self: Value) -> Value {
    unsafe {
        let d = &*doc(rb_self);
        let set = mkr_node_set_new(node_document(rb_self).as_raw());
        let id = unwrap(rb_self);
        if d.type_(id) == T_ELEMENT {
            let mut a = d.attrs(id);
            while let Some(at) = a {
                mkr_node_set_push(set, at.to_token() as *mut core::ffi::c_void);
                a = d.next(at);
            }
        }
        Value::from_raw(set)
    }
}
