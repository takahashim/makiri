//! The shared, representation-neutral node core (glue/ruby_node.c).
//!
//! HTML (Lexbor) and XML (custom-arena) nodes are two representations of one
//! Ruby-facing Node. The TypedData types, their GC functions and the raw
//! accessors moved to the Ruby <-> Lexbor seam ([`crate::bridge::lexbor`]);
//! what is left is representation-neutral and safe: the identity methods
//! (`==`/`eql?`, `hash`, `pointer_id`), which depend only on the node pointer
//! and never dereference it.
//!
//! The raw accessors are re-exported for the glue modules that already name
//! them here.

#![forbid(unsafe_code)]

use magnus::{Integer, Ruby, Value};

use crate::init::CLASS_NODE;

use crate::bridge::lexbor::node_identity;

/// Pointer identity: equal iff both wrappers resolve to the same node pointer,
/// so an HTML node is never equal to an XML one.
pub fn node_equals(rb_self: Value, other: Value) -> Result<bool, magnus::Error> {
    if !crate::bridge::ruby::is_kind_of(other, &CLASS_NODE) {
        return Ok(false);
    }
    Ok(node_identity(rb_self)? == node_identity(other)?)
}

/// Nokogiri-compatible identity: the underlying node pointer as an Integer.
/// Stable for the node's lifetime and unique among currently-live nodes; a
/// freed-then-reallocated node may reuse an address (the same caveat as
/// `Nokogiri::XML::Node#pointer_id`). `a.pointer_id == b.pointer_id` iff
/// `a.eql?(b)`.
pub fn node_pointer_id(ruby: &Ruby, rb_self: Value) -> Result<Integer, magnus::Error> {
    Ok(ruby.integer_from_u64(node_identity(rb_self)? as u64))
}

/// A stable hash from the node pointer, so `a == b` implies `a.hash == b.hash`
/// even across separately-created wrappers. Shares the pointer value with
/// `#pointer_id`.
pub fn node_hash(ruby: &Ruby, rb_self: Value) -> Result<Integer, magnus::Error> {
    node_pointer_id(ruby, rb_self)
}
