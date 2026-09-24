//! The XML node's readers: name, namespace, DTD identifiers, content,
//! navigation and attributes.
//!
//! A node is an index-arena `NodeId`, so every reader resolves through its
//! Document. XML nodes never inherit the Lexbor HTML readers - those live on
//! `Makiri::HTML::NodeMethods` - so the method names here follow that module's,
//! one for one, where the two representations share a meaning.
//!
//! A node's naming and a DOCTYPE's ids are read by kind, through
//! `Document::name_parts` / `doctype_ids`: the arena stores a DOCTYPE's ids in
//! the name fields, and only those two know it.
//!
//! The arena is reached through `XmlSelf::doc_ref` and the NodeSet through its
//! safe fill handle, so every read below is safe; the attribute-name lookups
//! that need a Ruby string's bytes live in `bridge::xml::find_attribute`.

#![forbid(unsafe_code)]

use magnus::{prelude::*, Error, Ruby, Value};

use super::strings::{str_field, utf8};
use super::{wrap, XmlSelf};
use crate::bridge::node_set::node_set_with_fill;
use crate::xml::model::{Document as XmlDoc, NodeId, NodeType};

fn nil(ruby: &Ruby) -> Value {
    ruby.qnil().as_value()
}

/// Wrap an optional reached node under the receiver's Document (None -> nil).
fn wrap_rel(this: XmlSelf, rel: Option<NodeId>) -> Value {
    wrap(rel.unwrap_or(NodeId::INVALID), this.document)
}

/// A byte field as a String, or nil when there is none.
fn str_or_nil(ruby: &Ruby, bytes: Option<&[u8]>) -> Value {
    bytes.map_or_else(|| nil(ruby), |b| str_field(ruby, b))
}

fn is_element(d: &XmlDoc, id: NodeId) -> bool {
    d.type_(id) == Some(NodeType::Element)
}

/* ---- name ---- */

pub fn name(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        let id = this.id;
        if let Some(n) = d.name_parts(id) {
            return Ok(str_field(ruby, n.qname));
        }
        Ok(match d.type_(id) {
            /* A PI's target and a DOCTYPE's name are its `local`. */
            Some(NodeType::Pi | NodeType::Doctype) => str_field(ruby, d.local(id)),
            Some(NodeType::Text) => ruby.str_new("text").as_value(),
            Some(NodeType::CData) => ruby.str_new("#cdata-section").as_value(),
            Some(NodeType::Comment) => ruby.str_new("comment").as_value(),
            Some(NodeType::Fragment) => ruby.str_new("#document-fragment").as_value(),
            _ => ruby.str_new("document").as_value(),
        })
    })
}

/// `#local_name`: Element and Attribute only.
pub fn local_name(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(str_or_nil(
            ruby,
            this.doc_ref().name_parts(this.id).map(|n| n.local),
        ))
    })
}

/// `#prefix`: nil when unprefixed - the distinction `#namespace` depends on -
/// and for any kind but Element and Attribute.
pub fn prefix(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(str_or_nil(
            ruby,
            this.doc_ref().name_parts(this.id).and_then(|n| n.prefix),
        ))
    })
}

/// `#namespace_uri`: nil in no namespace, and for any kind but Element and
/// Attribute.
pub fn namespace_uri(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(str_or_nil(
            ruby,
            this.doc_ref().name_parts(this.id).and_then(|n| n.ns_uri),
        ))
    })
}

/// `Element#tag_name` (DOM `tagName`): the qualified name - XML keeps its case
/// - or nil for a non-element.
pub fn tag_name(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        let tag = d.name_parts(this.id).filter(|_| is_element(d, this.id));
        Ok(str_or_nil(ruby, tag.map(|n| n.qname)))
    })
}

/// `ProcessingInstruction#target`, or nil for a non-PI.
pub fn pi_target(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        let target = (d.type_(this.id) == Some(NodeType::Pi)).then(|| d.local(this.id));
        Ok(str_or_nil(ruby, target))
    })
}

pub fn node_type(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        let ty = d.type_(this.id).map_or(0, |t| t.as_u32());
        Ok(ruby.integer_from_i64(ty as i64).as_value())
    })
}

/* ---- DTD identifiers ----
 *
 * An omitted id answers nil; an empty literal (`PUBLIC ""`) answers `""`. */

pub fn dtd_external_id(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(str_or_nil(
            ruby,
            this.doc_ref()
                .doctype_ids(this.id)
                .and_then(|ids| ids.public),
        ))
    })
}

pub fn dtd_system_id(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        Ok(str_or_nil(
            ruby,
            this.doc_ref()
                .doctype_ids(this.id)
                .and_then(|ids| ids.system),
        ))
    })
}

/* ---- content ---- */

/// `#content` / `#text` / `#inner_text`: a character-data node's own data, or
/// the concatenated text of every Text/CDATA descendant.
///
/// Measured first and built into one Ruby String of that size, so the copy is
/// Ruby's allocation - a failure is `NoMemoryError`, not a Rust abort - and the
/// arena bytes, which a GC does not move, are the only thing read across it.
pub fn content(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
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
            return Ok(str_field(ruby, d.value(id)));
        }

        let texts = || {
            core::iter::successors(d.first_child(id), move |&n| d.preorder_next(id, n))
                .filter(|&n| matches!(d.type_(n), Some(NodeType::Text | NodeType::CData)))
                .map(|n| d.value(n))
        };
        let total = texts().try_fold(0usize, |acc, t| acc.checked_add(t.len()));
        let Some(total) = total else {
            return Err(crate::bridge::ruby::makiri_error("text content too large"));
        };
        let out = ruby.str_with_capacity(total);
        for t in texts() {
            out.cat(t);
        }
        Ok(out.as_value())
    })
}

/// `#value`: an attribute's value; for any other node its text content, as
/// the HTML `#value` answers.
pub fn value(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        if d.type_(this.id) == Some(NodeType::Attribute) {
            return Ok(str_field(ruby, d.value(this.id)));
        }
        content(ruby, this)
    })
}

/* ---- navigation ---- */

pub fn parent(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(wrap_rel(this, this.doc_ref().parent(this.id))))
}
pub fn next(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(wrap_rel(this, this.doc_ref().next(this.id))))
}
pub fn previous(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(wrap_rel(this, this.doc_ref().prev(this.id))))
}
pub fn first_child(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(wrap_rel(this, this.doc_ref().first_child(this.id))))
}

/// The first element from `start` along `step`.
fn first_element(
    d: &XmlDoc,
    start: Option<NodeId>,
    step: impl Fn(NodeId) -> Option<NodeId>,
) -> Option<NodeId> {
    core::iter::successors(start, |&n| step(n)).find(|&n| is_element(d, n))
}

pub fn next_element(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        Ok(wrap_rel(
            this,
            first_element(d, d.next(this.id), |n| d.next(n)),
        ))
    })
}
pub fn previous_element(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        Ok(wrap_rel(
            this,
            first_element(d, d.prev(this.id), |n| d.prev(n)),
        ))
    })
}
pub fn first_element_child(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        Ok(wrap_rel(
            this,
            first_element(d, d.first_child(this.id), |n| d.next(n)),
        ))
    })
}
pub fn last_element_child(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        Ok(wrap_rel(
            this,
            first_element(d, d.last_child(this.id), |n| d.prev(n)),
        ))
    })
}

pub fn get_document(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| Ok(this.document))
}

/// Collect nodes into a NodeSet.
fn set_of(this: XmlSelf, nodes: impl Iterator<Item = NodeId>) -> Result<Value, Error> {
    let (set, fill) = node_set_with_fill(this.document);
    for id in nodes {
        fill.push(id.to_token() as *mut core::ffi::c_void)?;
    }
    Ok(set)
}

fn children_of(d: &XmlDoc, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    core::iter::successors(d.first_child(id), move |&n| d.next(n))
}

/// `#children`: every child node.
pub fn children(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| set_of(this, children_of(this.doc_ref(), this.id)))
}

/// `#element_children` / `#elements`: the child elements only.
pub fn element_children(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        set_of(this, children_of(d, this.id).filter(|&n| is_element(d, n)))
    })
}

/* ---- attributes ---- */

/// The receiver's attribute nodes, in document order; none for a non-element.
fn attrs_of(d: &XmlDoc, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let first = is_element(d, id).then(|| d.attrs(id)).flatten();
    core::iter::successors(first, move |&a| d.next(a))
}

/// `#[]` - the attribute's value, or nil.
pub fn aref(ruby: &Ruby, this: XmlSelf, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let found = crate::bridge::xml::find_attribute(this, rb_name)?;
        Ok(str_or_nil(ruby, found.map(|at| this.doc_ref().value(at))))
    })
}

/// The Attr NODE with that qualified name.
pub fn attribute_by_qualified_name(this: XmlSelf, rb_name: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let a = crate::bridge::xml::find_attribute(this, rb_name)?;
        Ok(wrap_rel(this, a))
    })
}

pub fn attribute_nodes(this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| set_of(this, attrs_of(this.doc_ref(), this.id)))
}

/// `#keys` -> the attribute names, in document order.
pub fn keys(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        let ary = ruby.ary_new();
        for at in attrs_of(d, this.id) {
            ary.push(str_field(ruby, d.qname(at)))?;
        }
        Ok(ary.as_value())
    })
}

/// `#values` -> the attribute values, in document order.
pub fn values(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let d = this.doc_ref();
        let ary = ruby.ary_new();
        for at in attrs_of(d, this.id) {
            ary.push(utf8(ruby, d.value(at)))?;
        }
        Ok(ary.as_value())
    })
}

/// `#<=>`: document (pre-order) position, as the HTML one - nil for a
/// non-node, a node of another document (an HTML one included), an attribute,
/// or two nodes in different trees. Comparable, which `Makiri::Node`
/// includes, supplies `<`, `>` and the rest.
pub fn spaceship(ruby: &Ruby, this: XmlSelf, other: Value) -> Result<Value, Error> {
    crate::bridge::ruby::entry(|| {
        let same_document = crate::bridge::ruby::is_kind_of(other, &crate::init::CLASS_NODE)
            && crate::bridge::wrapper::keepalive_document(other)?.equal(this.document)?;
        if !same_document {
            return Ok(ruby.qnil().as_value());
        }
        let order = crate::xml::xpath::document_order(
            this.doc_ref(),
            this.id,
            crate::bridge::xml::unwrap(other)?,
        );
        Ok(match order {
            Some(o) => ruby.integer_from_i64(o as i64).as_value(),
            None => ruby.qnil().as_value(),
        })
    })
}
