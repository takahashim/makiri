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
use crate::xml::model::{ArenaKind, Document as XmlDoc, NodeId};

/// `Makiri::XML::Namespace`, the Ruby `Data` class the queries make.
fn ns_class() -> Result<RClass, Error> {
    MOD_XML.defined()?.const_get("Namespace")
}

/// `Makiri::XML::Namespace.new(prefix, href)`: Ruby code, which may edit the
/// document - so no borrow of its arena may be held across the call.
fn new_ns(class: RClass, prefix: Value, href: Value) -> Result<Value, Error> {
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
    (d.type_(id) == Some(ArenaKind::Element))
        .then(|| d.attributes(id))
        .into_iter()
        .flatten()
        .filter_map(move |a| {
            let p = crate::xml::qname::xmlns_prefix(d.qname(a))?;
            Some((a, p, d.value(a)))
        })
}

/// `#namespace` - the node's own resolved namespace, or nil.
pub fn namespace(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let parts = this.doc_ref().name_parts(this.id);
        /* Both copied out before the call, so the arena is not lent across it. */
        let Some((prefix, uri)) = parts
            .and_then(|n| Some((n.prefix, n.ns_uri?)))
            .map(|(p, u)| (prefix_value(ruby, p), str_field(ruby, u)))
        else {
            return Ok(ruby.qnil().as_value());
        };
        new_ns(ns_class()?, prefix, uri)
    })
}

/// `#namespace_definitions` - the declarations made ON this element.
pub fn namespace_definitions(ruby: &Ruby, this: XmlSelf) -> Result<RArray, Error> {
    crate::bridge::ruby::entry(|| {
        /* Every (prefix, uri) copied into Ruby Strings FIRST: `Namespace.new`
         * is Ruby code that may edit this document, and the declarations are
         * read straight out of its arena. The pairs live in a Ruby Array, not
         * a Vec, so the GC sees them. */
        let pairs = ruby.ary_new();
        for (_, p, u) in declarations(this.doc_ref(), this.id) {
            pairs.push(prefix_value(ruby, Some(p)))?;
            pairs.push(utf8(ruby, u).as_value())?;
        }
        let class = ns_class()?;
        let arr = ruby.ary_new_capa(pairs.len() / 2);
        for i in (0..pairs.len()).step_by(2) {
            let (prefix, uri): (Value, Value) =
                (pairs.entry(i as isize)?, pairs.entry(i as isize + 1)?);
            arr.push(new_ns(class, prefix, uri)?)?;
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
