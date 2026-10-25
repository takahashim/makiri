//! Namespace introspection, Nokogiri-compatible.
//!
//! xmlns declarations are stored as ordinary attribute nodes - qname `xmlns` or
//! `xmlns:PREFIX` - so all four queries below are tree reads. What they hand
//! back is `Makiri::XML::Namespace`, a `Data` value object defined in Ruby
//! (`lib/makiri/xml/namespace.rb`); this module only makes them.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, RArray, RClass, RHash, Ruby, Value};

use super::XmlSelf;
use crate::bridge::build::{ary, aset, hash, push};
use crate::bridge::string::utf8_str;
use crate::init::MOD_XML;
use crate::xml::model::{ArenaKind, Document as XmlDoc, NodeId};

/// `Makiri::XML::Namespace`, the Ruby `Data` class the queries make.
fn ns_class(ruby: &Ruby) -> Result<RClass, Error> {
    let v = MOD_XML.defined(ruby)?.const_get(ruby, "Namespace")?;
    RClass::from_value(v)
        .ok_or_else(|| crate::bridge::ruby::internal_error("Makiri::XML::Namespace is not a class"))
}

/// `Makiri::XML::Namespace.new(prefix, href)`: Makiri's own Data class, but
/// Ruby code all the same, so no borrow of the arena is held across the call.
/// One of the two calls `runs_callers_ruby` (`rake dylint`) allows.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        runs_callers_ruby,
        reason = "Makiri's own Data class, called with no borrow of the arena held"
    )
)]
fn new_ns(ruby: &Ruby, class: RClass, prefix: Value, href: Value) -> Result<Value, Error> {
    class.call_method(ruby, "new", &[prefix, href])
}

/// A prefix as Ruby sees it: nil for none (the default namespace), else the
/// String.
fn prefix_value(ruby: &Ruby, p: Option<&[u8]>) -> Result<Value, Error> {
    match p.filter(|p| !p.is_empty()) {
        Some(p) => utf8_str(ruby, p),
        None => Ok(ruby.qnil().as_value()),
    }
}

/// The xmlns declarations among `id`'s attributes, as (declaring attribute,
/// declared prefix - empty for the default - and URI). None for a non-element.
fn declarations(d: &XmlDoc, id: NodeId) -> impl Iterator<Item = (NodeId, &[u8], &[u8])> + '_ {
    (d.type_(id) == Some(ArenaKind::Element))
        .then(|| d.attributes(id))
        .into_iter()
        .flatten()
        .filter_map(move |a| {
            let p = d.decl_prefix(a)?;
            Some((a, p, d.value(a)))
        })
}

/// `#namespace` - the node's own resolved namespace, or nil.
pub fn namespace(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    /* Both copied out before the call, so the arena is not lent across
     * it. */
    let pair = {
        let d = this.read()?;
        match d
            .name_parts(this.id)
            .and_then(|n| Some((n.prefix, n.ns_uri?)))
        {
            Some((p, u)) => Some((prefix_value(ruby, p)?, utf8_str(ruby, u)?)),
            None => None,
        }
    };
    let Some((prefix, uri)) = pair else {
        return Ok(ruby.qnil().as_value());
    };
    new_ns(ruby, ns_class(ruby)?, prefix, uri)
}

/// `#namespace_definitions` - the declarations made ON this element.
pub fn namespace_definitions(ruby: &Ruby, this: XmlSelf) -> Result<RArray, Error> {
    /* Every (prefix, uri) copied into Ruby Strings FIRST: `Namespace.new`
     * is Ruby code that may edit this document, and the declarations are
     * read straight out of its arena. The pairs live in a Ruby Array, not
     * a Vec, so the GC sees them. */
    let pairs = {
        let d = this.read()?;
        let pairs = ary(ruby, 0)?;
        for (_, p, u) in declarations(&d, this.id) {
            push(pairs, prefix_value(ruby, Some(p))?)?;
            push(pairs, utf8_str(ruby, u)?)?;
        }
        pairs
    };
    let class = ns_class(ruby)?;
    let arr = ary(ruby, pairs.len() / 2)?;
    for i in (0..pairs.len()).step_by(2) {
        let (prefix, uri): (Value, Value) = (pairs.entry(i), pairs.entry(i + 1));
        push(arr, new_ns(ruby, class, prefix, uri)?)?;
    }
    Ok(arr)
}

/// `#namespaces` - every declaration in scope here, keyed by the declaring
/// attribute's name. The inner scope wins because the first binding seen is kept.
pub fn namespaces(ruby: &Ruby, this: XmlSelf) -> Result<RHash, Error> {
    let d = this.read()?;
    /* Built with the read held: its keys are Strings, so building it runs
     * no Ruby of the caller's, and a failed allocation is `Err`. */
    let h = hash(ruby)?;
    for id in core::iter::successors(Some(this.id), |&n| d.parent(n)) {
        for (at, _, u) in declarations(&d, id) {
            let key = crate::bridge::string::utf8_rstring(ruby, d.qname(at))?;
            if h.get(key)?.is_none() {
                aset(h, key.as_value(), utf8_str(ruby, u)?)?;
            }
        }
    }
    Ok(h)
}

/// `#collect_namespaces` - every declaration anywhere in the document, in
/// document order, so a later one with the same name wins.
pub fn collect_namespaces(ruby: &Ruby, this: XmlSelf) -> Result<RHash, Error> {
    let d = this.read()?;
    let top = core::iter::successors(Some(this.id), |&n| d.parent(n))
        .last()
        .unwrap_or(this.id);
    /* As `namespaces`: built with the read held. */
    let h = hash(ruby)?;
    for id in core::iter::successors(Some(top), |&n| d.preorder_next(top, n)) {
        for (at, _, u) in declarations(&d, id) {
            aset(h, utf8_str(ruby, d.qname(at))?, utf8_str(ruby, u)?)?;
        }
    }
    Ok(h)
}
