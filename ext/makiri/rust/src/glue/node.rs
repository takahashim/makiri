//! The node identity methods both representations share.
//!
//! HTML (Lexbor) and XML (custom-arena) nodes are two representations of one
//! Ruby-facing Node. `==`/`eql?`, `hash` and `pointer_id` never dereference
//! the node, so one implementation serves both; the TypedData types behind it
//! are [`crate::bridge::wrapper`]'s.

#![forbid(unsafe_code)]

use magnus::{Integer, Ruby, Value};

use crate::init::CLASS_NODE;

use crate::bridge::wrapper::{node_identity, node_key};

/// Node identity: equal iff both wrappers name the same node of the same
/// representation, so an HTML node is never equal to an XML one (see
/// [`node_key`]).
pub fn node_equals(rb_self: Value, other: Value) -> Result<bool, magnus::Error> {
    crate::bridge::ruby::entry(|| {
        if !crate::bridge::ruby::is_kind_of(other, &CLASS_NODE) {
            return Ok(false);
        }
        Ok(node_key(rb_self)? == node_key(other)?)
    })
}

/// Nokogiri-compatible identity: the underlying node pointer as an Integer.
/// Stable for the node's lifetime and unique among currently-live nodes; a
/// freed-then-reallocated node may reuse an address (the same caveat as
/// `Nokogiri::XML::Node#pointer_id`). `a.pointer_id == b.pointer_id` iff
/// `a.eql?(b)`.
pub fn node_pointer_id(ruby: &Ruby, rb_self: Value) -> Result<Integer, magnus::Error> {
    crate::bridge::ruby::entry(|| Ok(ruby.integer_from_u64(node_identity(rb_self)? as u64)))
}

/// A stable hash from the node word, so `a == b` implies `a.hash == b.hash`
/// even across separately-created wrappers. Shares the value with
/// `#pointer_id`; an HTML and an XML node may share it, which a hash allows.
pub fn node_hash(ruby: &Ruby, rb_self: Value) -> Result<Integer, magnus::Error> {
    crate::bridge::ruby::entry(|| node_pointer_id(ruby, rb_self))
}
