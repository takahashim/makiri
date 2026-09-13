//! The XML node's readers: name, namespace, DTD identifiers, content,
//! navigation and attributes (glue/ruby_xml_node.c, first third).
//!
//! Every one of these reads the arena node's fields directly. XML nodes never
//! inherit the Lexbor HTML readers - those live on `Makiri::HTML::NodeMethods` -
//! so this surface is structural rather than shared.

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, Ruby, Value};

use super::abi::*;
use super::{node_document, unwrap};

/* ---- name ---- */

/// `#name`. Elements and attributes answer with their qualified name; the other
/// types answer with the DOM's node name for their kind, except a PI, whose
/// name is its target, and a doctype, whose name is the DOCTYPE name.
pub fn name(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let n = &*unwrap(rb_self);
        match n.type_ {
            T_ELEMENT | T_ATTRIBUTE => str_field(ruby, n.qname, n.qname_len),
            T_PI | T_DOCTYPE => str_field(ruby, n.local, n.local_len),
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
        let n = &*unwrap(rb_self);
        if n.type_ == T_ELEMENT || n.type_ == T_ATTRIBUTE {
            return str_field(ruby, n.local, n.local_len);
        }
        ruby.qnil().as_value()
    }
}

/// `#prefix`. A zero-length prefix means unprefixed, which is nil rather than
/// `""` - the distinction `#namespace` depends on.
pub fn prefix(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let n = &*unwrap(rb_self);
        if n.prefix_len == 0 {
            return ruby.qnil().as_value();
        }
        str_field(ruby, n.prefix, n.prefix_len)
    }
}

pub fn namespace_uri(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let n = &*unwrap(rb_self);
        if n.ns_uri_len == 0 {
            return ruby.qnil().as_value();
        }
        str_field(ruby, n.ns_uri, n.ns_uri_len)
    }
}

pub fn node_type(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        ruby.integer_from_i64((*unwrap(rb_self)).type_ as i64)
            .as_value()
    }
}

/* ---- DTD identifiers ----
 *
 * The doctype node repurposes fields: local and qname are the DOCTYPE name
 * (`#name`), prefix is the PUBLIC/external id, value the SYSTEM id. A NULL field
 * means that id was absent and answers nil; an empty literal (`PUBLIC ""`) is a
 * non-NULL zero-length slice and answers `""`. Mirrors Nokogiri's
 * `DTD#external_id` / `#system_id`, with `#public_id` as the WHATWG-DOM-style
 * alias. The DTD body is NOT parsed, so there is nothing else to read. */

pub fn dtd_external_id(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let n = &*unwrap(rb_self);
        str_field_or_nil(ruby, n.prefix, n.prefix_len)
    }
}

pub fn dtd_system_id(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let n = &*unwrap(rb_self);
        str_field_or_nil(ruby, n.value, n.value_len)
    }
}

/* ---- content ---- */

/// `#content` / `#text` / `#inner_text`.
///
/// A leaf data node answers with its own value verbatim. An element or document
/// concatenates every TEXT and CDATA descendant in document order, walked
/// iteratively through the parent pointers - no recursion, so a deep tree cannot
/// overflow the stack.
pub fn content(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let node = unwrap(rb_self);
        let n = &*node;
        if matches!(n.type_, T_TEXT | T_CDATA | T_COMMENT | T_ATTRIBUTE | T_PI) {
            return str_field(ruby, n.value, n.value_len);
        }

        let mut out: Vec<u8> = Vec::new();
        let mut cur = n.first_child;
        while !cur.is_null() {
            let c = &*cur;
            if matches!(c.type_, T_TEXT | T_CDATA) && c.value_len > 0 {
                out.extend_from_slice(core::slice::from_raw_parts(
                    c.value as *const u8,
                    c.value_len as usize,
                ));
            }
            if !c.first_child.is_null() {
                cur = c.first_child;
                continue;
            }
            while !cur.is_null() && cur != node && (*cur).next.is_null() {
                cur = (*cur).parent;
            }
            if cur.is_null() || cur == node {
                break;
            }
            cur = (*cur).next;
        }
        utf8(ruby, &out).as_value()
    }
}

pub fn value(ruby: &Ruby, rb_self: Value) -> Value {
    unsafe {
        let n = &*unwrap(rb_self);
        str_field(ruby, n.value, n.value_len)
    }
}

/* ---- navigation ---- */

/// Wrap a node reached from `rb_self`, under `rb_self`'s Document.
unsafe fn wrap_rel(rb_self: Value, rel: *mut Node) -> Value {
    super::wrap(rel, node_document(rb_self))
}

pub fn parent(rb_self: Value) -> Value {
    unsafe { wrap_rel(rb_self, (*unwrap(rb_self)).parent) }
}
pub fn next(rb_self: Value) -> Value {
    unsafe { wrap_rel(rb_self, (*unwrap(rb_self)).next) }
}
pub fn previous(rb_self: Value) -> Value {
    unsafe { wrap_rel(rb_self, (*unwrap(rb_self)).prev) }
}
pub fn first_child(rb_self: Value) -> Value {
    unsafe { wrap_rel(rb_self, (*unwrap(rb_self)).first_child) }
}
pub fn last_child(rb_self: Value) -> Value {
    unsafe { wrap_rel(rb_self, (*unwrap(rb_self)).last_child) }
}

pub fn get_document(rb_self: Value) -> Value {
    unsafe { node_document(rb_self) }
}

/// `#element_children` - the child ELEMENT nodes only, in document order (the
/// counterpart of HTML's).
pub fn element_children(rb_self: Value) -> Value {
    unsafe {
        let doc = node_document(rb_self);
        let set = mkr_node_set_new(doc.as_raw());
        let mut c = (*unwrap(rb_self)).first_child;
        while !c.is_null() {
            if (*c).type_ == T_ELEMENT {
                mkr_node_set_push(set, c as *mut core::ffi::c_void);
            }
            c = (*c).next;
        }
        Value::from_raw(set)
    }
}

pub fn children(rb_self: Value) -> Value {
    unsafe {
        let doc = node_document(rb_self);
        let set = mkr_node_set_new(doc.as_raw());
        let mut c = (*unwrap(rb_self)).first_child;
        while !c.is_null() {
            mkr_node_set_push(set, c as *mut core::ffi::c_void);
            c = (*c).next;
        }
        Value::from_raw(set)
    }
}

/* ---- attributes ---- */

/// The attribute of `n` whose qualified name is exactly `name`.
///
/// XML attributes are stored under their qualified name, so this is the match
/// `#[]` makes. `set_attribute_ns` can leave two attributes sharing a qualified
/// name in different namespaces; the first wins, as `getAttribute` does.
unsafe fn find_attr(n: *const Node, name: &[u8]) -> *mut Node {
    if (*n).type_ != T_ELEMENT {
        return core::ptr::null_mut();
    }
    let mut a = (*n).attrs;
    while !a.is_null() {
        let at = &*a;
        if !at.qname.is_null()
            && core::slice::from_raw_parts(at.qname as *const u8, at.qname_len as usize) == name
        {
            return a;
        }
        a = at.next;
    }
    core::ptr::null_mut()
}

/// `#[]` - the attribute's value, or nil.
pub fn aref(ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    unsafe {
        let n = unwrap(rb_self);
        if (*n).type_ != T_ELEMENT {
            return Ok(ruby.qnil().as_value());
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let a = find_attr(n, nv.bytes());
        /* Keep the name's String reachable until the comparison is done. */
        core::hint::black_box(rb_name);
        if a.is_null() {
            return Ok(ruby.qnil().as_value());
        }
        Ok(str_field(ruby, (*a).value, (*a).value_len))
    }
}

/// The Attr NODE with that qualified name, which is what the DOM's by-name
/// family needs to read the attribute's namespace and prefix. (The HTML side
/// has to look harder, and carries the note on case.)
pub fn attribute_by_qualified_name(
    ruby: &Ruby,
    rb_self: Value,
    rb_name: Value,
) -> Result<Value, Error> {
    unsafe {
        let n = unwrap(rb_self);
        if (*n).type_ != T_ELEMENT {
            return Ok(ruby.qnil().as_value());
        }
        let nv = mkr_ruby_verified_text(rb_name.as_raw(), c"attribute name".as_ptr());
        let a = find_attr(n, nv.bytes());
        core::hint::black_box(rb_name);
        Ok(super::wrap(a, node_document(rb_self)))
    }
}

/// Exists so the DOM layer can ask both representations the same question; for
/// XML it is already the same match `#[]` makes.
pub fn attribute_value_by_qualified_name(
    ruby: &Ruby,
    rb_self: Value,
    rb_name: Value,
) -> Result<Value, Error> {
    aref(ruby, rb_self, rb_name)
}

pub fn attribute_nodes(rb_self: Value) -> Value {
    unsafe {
        let doc = node_document(rb_self);
        let set = mkr_node_set_new(doc.as_raw());
        let n = unwrap(rb_self);
        if (*n).type_ == T_ELEMENT {
            let mut a = (*n).attrs;
            while !a.is_null() {
                mkr_node_set_push(set, a as *mut core::ffi::c_void);
                a = (*a).next;
            }
        }
        Value::from_raw(set)
    }
}
