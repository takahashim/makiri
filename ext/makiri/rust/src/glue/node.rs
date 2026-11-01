//! The node methods both representations share - identity, the child list
//! readers over `ChildListHost` and the Document's edit versions.
//!
//! HTML and XML nodes, each in its own index arena, are two representations
//! of one Ruby-facing Node. `==`/`eql?`, `hash` and `pointer_id` never dereference
//! the node, so one implementation serves both; the TypedData types behind it
//! are [`crate::bridge::wrapper`]'s.

#![forbid(unsafe_code)]

use crate::bridge::rb::{Ruby, Value};

use crate::init::CLASS_NODE;

/// `remove` / `unlink` on a Document, on either representation: the DOM gives
/// one no `remove()`, and it is in no tree to leave (`dom_rules::removable`).
pub const DOCUMENT_NOT_REMOVABLE: &str = "cannot remove the document node";

use crate::bridge::wrapper::{node_identity, same_node, ChildListHost};
use crate::child_index::ChildList;

/// Node identity: equal iff both wrappers name the same node of the same
/// Document, so an HTML node is never equal to an XML one, nor a node to one
/// of another document that happens to share its word (see [`same_node`]).
pub fn node_equals(
    ruby: &Ruby,
    rb_self: Value,
    other: Value,
) -> Result<bool, crate::bridge::rb::Error> {
    if !crate::bridge::ruby::is_kind_of(ruby, other, &CLASS_NODE) {
        return Ok(false);
    }
    same_node(ruby, rb_self, other)
}

/// Nokogiri-compatible identity: the underlying node pointer as an Integer.
/// Stable for the node's lifetime and unique among currently-live nodes; a
/// freed-then-reallocated node may reuse an address (the same caveat as
/// `Nokogiri::XML::Node#pointer_id`). `a.pointer_id == b.pointer_id` iff
/// `a.eql?(b)`.
pub fn node_pointer_id(ruby: &Ruby, rb_self: Value) -> Result<Value, crate::bridge::rb::Error> {
    crate::bridge::rb::IntoValue::into_value(node_identity(ruby, rb_self)?, ruby)
}

/// A stable hash from the node word, so `a == b` implies `a.hash == b.hash`
/// even across separately-created wrappers. Shares the value with
/// `#pointer_id`; an HTML and an XML node may share it, which a hash allows.
pub fn node_hash(ruby: &Ruby, rb_self: Value) -> Result<Value, crate::bridge::rb::Error> {
    node_pointer_id(ruby, rb_self)
}

/// A `child_at` / `element_child_at` index argument: None (so nil) for a
/// negative one, or one too large for any list. Not an Integer is a
/// `TypeError` (`bridge::ruby::integer_arg`): no `to_int` is called.
fn child_index_arg(i: Value) -> Result<Option<usize>, crate::bridge::rb::Error> {
    let i = crate::bridge::ruby::integer_arg(i)?;
    i.to_usize_within()
}

/// `#child_count` (DOM `childNodes.length`): every child node, counted
/// without building `#children`.
pub fn child_count<S: ChildListHost>(this: S) -> Result<usize, crate::bridge::rb::Error> {
    this.child_count(ChildList::Nodes)
}

/// `#element_child_count` (DOM `childElementCount`).
pub fn element_child_count<S: ChildListHost>(this: S) -> Result<usize, crate::bridge::rb::Error> {
    this.child_count(ChildList::Elements)
}

/// `#child_at(i)` (DOM `childNodes[i]`): the child node at `i`, or nil when
/// `i` is negative or past the end.
pub fn child_at<S: ChildListHost>(
    ruby: &Ruby,
    this: S,
    i: Value,
) -> Result<Option<Value>, crate::bridge::rb::Error> {
    match child_index_arg(i)? {
        Some(i) => this.child_at(ruby, ChildList::Nodes, i),
        None => Ok(None),
    }
}

/// `#element_child_at(i)` (DOM `children[i]`): the child element at `i`, or
/// nil.
pub fn element_child_at<S: ChildListHost>(
    ruby: &Ruby,
    this: S,
    i: Value,
) -> Result<Option<Value>, crate::bridge::rb::Error> {
    match child_index_arg(i)? {
        Some(i) => this.child_at(ruby, ChildList::Elements, i),
        None => Ok(None),
    }
}

/// `Document#tree_version`: an Integer that grows with every edit that can
/// change a child list of a node the document owns (attached, detached or in
/// a fragment) - add, remove, replace, `inner_html=`, an element's
/// `content=` and the like, and both documents of a move between them.
/// Attribute edits (`attribute_version`'s) and character-data edits (a Text,
/// Comment, CDATA or PI node's `content=`) leave it alone. A reader caching a child list keys it by this.
pub fn document_tree_version(rb_self: Value) -> Result<u64, crate::bridge::rb::Error> {
    crate::bridge::wrapper::tree_version(rb_self)
}

/// `Document#attribute_version`: an Integer that grows with every edit of an
/// attribute of an element the document owns - added, removed, its value set
/// (to the same value too), an Attr's `content=` included. Child-list edits
/// (`tree_version`'s) and character-data edits leave it alone. A reader
/// caching what depends on attributes keys it by this.
pub fn document_attribute_version(rb_self: Value) -> Result<u64, crate::bridge::rb::Error> {
    crate::bridge::wrapper::attribute_version(rb_self)
}
