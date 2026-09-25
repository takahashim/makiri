//! Namespace introspection, Nokogiri-compatible.
//!
//! xmlns declarations are stored as ordinary attribute nodes - qname `xmlns` or
//! `xmlns:PREFIX` - so all four queries below are tree reads. What they hand
//! back is `Makiri::XML::Namespace`, a `Data` value object defined in Ruby
//! (`lib/makiri/xml/namespace.rb`); this module only makes them.
//!
//! Each query walks a tree built from input, so each runs under
//! `bridge::ruby::entry`, which turns a panic into `Makiri::InternalError`.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, RArray, RClass, RHash, Ruby, Value};

use super::strings::{str_field, utf8};
use super::XmlSelf;
use crate::init::MOD_XML;
use crate::xml::model::{Document as XmlDoc, NodeId, NodeType};

/// `Makiri::XML::Namespace.new(prefix, href)`.
fn new_ns(prefix: Value, href: Value) -> Result<Value, Error> {
    let xml = MOD_XML.module();
    let class: RClass = xml.const_get("Namespace")?;
    class.funcall("new", (prefix, href))
}

/// A prefix as Ruby sees it: nil for none (the default namespace), else the
/// String.
fn prefix_value(ruby: &Ruby, p: Option<&[u8]>) -> Value {
    p.filter(|p| !p.is_empty())
        .map_or_else(|| ruby.qnil().as_value(), |p| str_field(ruby, p))
}

/// The xmlns declarations among `id`'s attributes, as (declaring attribute,
/// declared prefix - empty for the default - and URI). None for a non-element.
fn declarations(d: &XmlDoc, id: NodeId) -> impl Iterator<Item = (NodeId, &[u8], &[u8])> + '_ {
    let first = (d.type_(id) == Some(NodeType::Element))
        .then(|| d.attrs(id))
        .flatten();
    core::iter::successors(first, move |&a| d.next(a)).filter_map(move |a| {
        let p = crate::xml::qname::xmlns_prefix(d.qname(a))?;
        Some((a, p, d.value(a)))
    })
}

/// `#namespace` - the node's own resolved namespace, or nil.
pub fn namespace(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let parts = this.doc_ref().name_parts(this.id);
        match parts.and_then(|n| Some((n.prefix, n.ns_uri?))) {
            Some((prefix, uri)) => new_ns(prefix_value(ruby, prefix), str_field(ruby, uri)),
            None => Ok(ruby.qnil().as_value()),
        }
    })
}

/// `#namespace_definitions` - the declarations made ON this element.
pub fn namespace_definitions(ruby: &Ruby, this: XmlSelf) -> Result<RArray, Error> {
    crate::bridge::ruby::entry(|| {
        let arr = ruby.ary_new();
        for (_, p, u) in declarations(this.doc_ref(), this.id) {
            arr.push(new_ns(
                prefix_value(ruby, Some(p)),
                utf8(ruby, u).as_value(),
            )?)?;
        }
        Ok(arr)
    })
}

/// `#namespaces` - every declaration in scope here, keyed by the declaring
/// attribute's name. The inner scope wins because the first binding seen is kept.
pub fn namespaces(ruby: &Ruby, this: XmlSelf) -> Result<RHash, Error> {
    crate::bridge::ruby::entry(|| {
        let h = ruby.hash_new();
        let d = this.doc_ref();
        for id in core::iter::successors(Some(this.id), |&n| d.parent(n)) {
            for (at, _, u) in declarations(d, id) {
                let key = str_field(ruby, d.qname(at));
                if h.get(key).is_none() {
                    h.aset(key, utf8(ruby, u))?;
                }
            }
        }
        Ok(h)
    })
}

/// `#collect_namespaces` - every declaration anywhere in the document, in
/// document order, so a later one with the same name wins.
pub fn collect_namespaces(ruby: &Ruby, this: XmlSelf) -> Result<RHash, Error> {
    crate::bridge::ruby::entry(|| {
        let h = ruby.hash_new();
        let d = this.doc_ref();
        let top = core::iter::successors(Some(this.id), |&n| d.parent(n))
            .last()
            .unwrap_or(this.id);
        for id in core::iter::successors(Some(top), |&n| d.preorder_next(top, n)) {
            for (at, _, u) in declarations(d, id) {
                h.aset(str_field(ruby, d.qname(at)), utf8(ruby, u))?;
            }
        }
        Ok(h)
    })
}
