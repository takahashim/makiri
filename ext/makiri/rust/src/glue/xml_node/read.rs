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
//! The arena is reached through `XmlSelf::read`, a guard. A node is wrapped
//! after the read (`XmlView::wrap`), and a String (`str_from_read`), a NodeSet,
//! an Array or a joined text is built with it held; every allocation
//! returns `Err` on failure (`nupur::raw`), so the guard drops either way. The
//! attribute-name lookups that need a Ruby string's bytes live in
//! `bridge::xml::find_attribute`.

#![forbid(unsafe_code)]

use crate::bridge::rb::{prelude::*, Error, Ruby, Value};

use super::{wrap, XmlSelf};
use crate::bridge::string::{str_from_read, utf8_str, StrKind};
use crate::child_index;
use crate::xml::model::{ArenaKind, Document as XmlDoc, NodeId};

/// Wrap an optional reached node under the receiver's Document (None -> nil).
fn wrap_rel(ruby: &Ruby, this: &XmlSelf, rel: Option<NodeId>) -> Result<Option<Value>, Error> {
    rel.map(|n| wrap(ruby, n, this.document())).transpose()
}

fn is_element(d: &XmlDoc, id: NodeId) -> bool {
    d.type_(id) == Some(ArenaKind::Element)
}

/* ---- name ---- */

pub fn name(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    let name = str_from_read(ruby, this.read()?, StrKind::Utf8, |d| {
        let id = d.id;
        if let Some(n) = d.name_parts(id) {
            return Some(n.qname);
        }
        Some(match d.type_(id) {
            /* A PI's target and a DOCTYPE's name are its `local`. */
            Some(ArenaKind::Pi | ArenaKind::DocumentType) => d.local(id),
            Some(ArenaKind::Text) => b"text",
            Some(ArenaKind::CDataSection) => b"#cdata-section",
            Some(ArenaKind::Comment) => b"comment",
            Some(ArenaKind::DocumentFragment) => b"#document-fragment",
            _ => b"document",
        })
    })?;
    match name {
        Some(name) => Ok(name),
        None => utf8_str(ruby, b"").map(|s| s.as_value()),
    }
}

/// `#local_name`: Element and Attribute only.
pub fn local_name(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::DomName, |d| {
        d.name_parts(d.id).map(|n| n.local)
    })
}

/// `#prefix`: nil when unprefixed - the distinction `#namespace` depends on -
/// and for any kind but Element and Attribute.
pub fn prefix(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::DomName, |d| {
        d.name_parts(d.id).and_then(|n| n.prefix)
    })
}

/// `#namespace_uri`: nil in no namespace, and for any kind but Element and
/// Attribute.
pub fn namespace_uri(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::DomName, |d| {
        d.name_parts(d.id).and_then(|n| n.ns_uri)
    })
}

/// `Element#tag_name` (DOM `tagName`): the qualified name - XML keeps its case
/// - or nil for a non-element.
pub fn tag_name(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::DomName, |d| {
        d.name_parts(d.id)
            .filter(|_| is_element(d, d.id))
            .map(|n| n.qname)
    })
}

/// `ProcessingInstruction#target`, or nil for a non-PI.
pub fn pi_target(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::Utf8, |d| {
        (d.type_(d.id) == Some(ArenaKind::Pi)).then(|| d.local(d.id))
    })
}

pub fn node_type(_ruby: &Ruby, this: XmlSelf) -> Result<u32, Error> {
    Ok(this.read()?.type_(this.id).map_or(0, |t| t as u32))
}

/* ---- DTD identifiers ----
 *
 * An omitted id answers nil; an empty literal (`PUBLIC ""`) answers `""`. */

pub fn dtd_external_id(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::Utf8, |d| {
        d.doctype_ids(d.id).and_then(|ids| ids.public)
    })
}

pub fn dtd_system_id(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    str_from_read(ruby, this.read()?, StrKind::Utf8, |d| {
        d.doctype_ids(d.id).and_then(|ids| ids.system)
    })
}

/* ---- content ---- */

/// `#content` / `#text` / `#inner_text`: a character-data node's own data, or
/// the concatenated text of every Text/CDATA descendant.
///
/// Measured first and built into one Ruby String of that size, so the copy is
/// Ruby's allocation - a failure is `NoMemoryError` (as `Err`), not a Rust
/// abort.
pub fn content(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    let d = this.read()?;
    let id = this.id;
    if matches!(
        d.type_(id),
        Some(
            ArenaKind::Text
                | ArenaKind::CDataSection
                | ArenaKind::Comment
                | ArenaKind::Attribute
                | ArenaKind::Pi
        )
    ) {
        let value = str_from_read(ruby, d, StrKind::Utf8, |d| Some(d.value(d.id)))?;
        return match value {
            Some(value) => Ok(value),
            None => utf8_str(ruby, b"").map(|s| s.as_value()),
        };
    }

    let texts = || {
        core::iter::successors(d.first_child(id), |&n| d.preorder_next(id, n))
            .filter(|&n| matches!(d.type_(n), Some(ArenaKind::Text | ArenaKind::CDataSection)))
            .map(|n| d.value(n))
    };
    let total = texts().try_fold(0usize, |acc, t| acc.checked_add(t.len()));
    let Some(total) = total else {
        return Err(crate::bridge::ruby::makiri_error("text content too large"));
    };
    crate::bridge::string::utf8_str_from_slices(ruby, texts(), total)
}

/* ---- navigation ---- */

pub fn parent(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, |d, id| d.parent(id))
}
pub fn next(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, |d, id| d.next(id))
}
pub fn previous(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, |d, id| d.prev(id))
}
pub fn first_child(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, |d, id| d.first_child(id))
}

pub fn last_child(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, |d, id| d.last_child(id))
}

/// `#root_node` (DOM `getRootNode()`) - see [`crate::dom_rules::root`]; an
/// Attr is its own root.
pub fn root_node(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    let root = crate::dom_rules::root(&*this.read()?, this.id);
    wrap(ruby, root, this.document())
}

pub fn next_element(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, child_index::next_element_sibling)
}
pub fn previous_element(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?
        .wrap(ruby, child_index::previous_element_sibling)
}
pub fn first_element_child(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, child_index::first_element_child)
}
pub fn last_element_child(ruby: &Ruby, this: XmlSelf) -> Result<Option<Value>, Error> {
    this.read()?.wrap(ruby, child_index::last_element_child)
}

pub fn get_document(this: XmlSelf) -> Result<Value, Error> {
    Ok(this.document())
}

fn children_of(d: &XmlDoc, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    core::iter::successors(d.first_child(id), move |&n| d.next(n))
}

/// `#children`: every child node.
pub fn children(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    this.read()?
        .set(ruby, |d, id, out| children_of(d, id).for_each(out))
}

/// `#_traverse_order`: this node and its descendants in post-order, as
/// HTML's.
pub fn traverse_order(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    this.read()?.set(ruby, |d, id, out| {
        crate::glue::node::post_order(
            id,
            |n| n == id,
            |n| d.first_child(n),
            |n| d.next(n),
            |n| d.parent(n),
            out,
        )
    })
}

/// `#ancestors`: the element ancestors, nearest first, the node itself
/// excluded - as HTML's.
pub fn ancestors(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    this.read()?.set(ruby, |d, id, out| {
        core::iter::successors(d.parent(id), |&p| d.parent(p))
            .filter(|&n| is_element(d, n))
            .for_each(out)
    })
}

/// `#element_children` / `#elements`: the child elements only.
pub fn element_children(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    this.read()?.set(ruby, |d, id, out| {
        children_of(d, id)
            .filter(|&n| is_element(d, n))
            .for_each(out)
    })
}

/* ---- attributes ---- */

/// The receiver's attribute nodes, in document order; none for a non-element.
fn attrs_of(d: &XmlDoc, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    is_element(d, id)
        .then(|| d.attributes(id))
        .into_iter()
        .flatten()
}

/// `#[]` - the attribute's value, or nil.
pub fn aref(ruby: &Ruby, this: XmlSelf, rb_name: Value) -> Result<Option<Value>, Error> {
    let found = crate::bridge::xml::find_attribute(ruby, &this, rb_name)?;
    str_from_read(ruby, this.read()?, StrKind::Utf8, |d| {
        found.map(|at| d.value(at))
    })
}

/// `#key?` / `#has_attribute?`: whether `#[]` finds an attribute by that
/// qualified name (DOM `hasAttribute`) - case-sensitive, as XML names are.
pub fn has_key(ruby: &Ruby, this: XmlSelf, rb_name: Value) -> Result<bool, Error> {
    Ok(crate::bridge::xml::find_attribute(ruby, &this, rb_name)?.is_some())
}

/// `#attribute_value_ns(ns, local)` (DOM `getAttributeNS`): the value of the
/// attribute in namespace `ns` (nil or "" for none) with local name `local`,
/// or nil - for a non-element too.
pub fn attribute_value_ns(
    ruby: &Ruby,
    this: XmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Option<Value>, Error> {
    let found = crate::bridge::xml::find_attribute_ns(ruby, &this, rb_ns, rb_local)?;
    str_from_read(ruby, this.read()?, StrKind::Utf8, |d| {
        found.map(|at| d.value(at))
    })
}

/// `#attribute_node_ns(ns, local)` (DOM `getAttributeNodeNS`): the Attr
/// [`attribute_value_ns`] reads, or nil.
pub fn attribute_node_ns(
    ruby: &Ruby,
    this: XmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Option<Value>, Error> {
    let found = crate::bridge::xml::find_attribute_ns(ruby, &this, rb_ns, rb_local)?;
    wrap_rel(ruby, &this, found)
}

/// The Attr NODE with that qualified name.
pub fn attribute_by_qualified_name(
    ruby: &Ruby,
    this: XmlSelf,
    rb_name: Value,
) -> Result<Option<Value>, Error> {
    let a = crate::bridge::xml::find_attribute(ruby, &this, rb_name)?;
    wrap_rel(ruby, &this, a)
}

pub fn attribute_nodes(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    this.read()?
        .set(ruby, |d, id, out| attrs_of(d, id).for_each(out))
}

/// `#keys` -> the attribute names, in document order.
pub fn keys(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    let d = this.read()?;
    let ary = crate::bridge::build::ary(ruby, 0)?;
    for at in attrs_of(&d, this.id) {
        crate::bridge::build::push(ary, utf8_str(ruby, d.qname(at))?)?;
    }
    Ok(ary.as_value())
}

/// `#values` -> the attribute values, in document order.
pub fn values(ruby: &Ruby, this: XmlSelf) -> Result<Value, Error> {
    let d = this.read()?;
    let ary = crate::bridge::build::ary(ruby, 0)?;
    for at in attrs_of(&d, this.id) {
        crate::bridge::build::push(ary, utf8_str(ruby, d.value(at))?)?;
    }
    Ok(ary.as_value())
}

/// `#<=>`: document (pre-order) position, as the HTML one - nil for a
/// non-node, a node of another document (an HTML one included), an attribute,
/// or two nodes in different trees. Comparable, which `Makiri::Node`
/// includes, supplies `<`, `>` and the rest.
pub fn spaceship(ruby: &Ruby, this: XmlSelf, other: Value) -> Result<Option<i64>, Error> {
    let same_document = crate::bridge::ruby::is_kind_of(ruby, other, &crate::init::CLASS_NODE)
        && crate::bridge::ruby::same_value(
            crate::bridge::wrapper::keepalive_document(ruby, other)?,
            this.document(),
        );
    if !same_document {
        return Ok(None);
    }
    let other = crate::bridge::xml::xml_node_unwrap(ruby, other)?;
    let order = crate::xml::xpath::document_order(&*this.read()?, this.id, other);
    Ok(order.map(|o| o as i64))
}
