//! The node identity methods both representations share, and the pieces of
//! the DOM's naming rules their factories raise alike.
//!
//! HTML (Lexbor) and XML (custom-arena) nodes are two representations of one
//! Ruby-facing Node. `==`/`eql?`, `hash` and `pointer_id` never dereference
//! the node, so one implementation serves both; the TypedData types behind it
//! are [`crate::bridge::wrapper`]'s.

#![forbid(unsafe_code)]

use magnus::{Error, Integer, Ruby, Value};

use crate::bridge::ruby::makiri_error;
use crate::xml::dom_name;

use crate::init::CLASS_NODE;

use crate::bridge::wrapper::{node_identity, node_key};

/// The refusal of a namespace that does not fit the qualified name it came with.
const NS_MISFIT: &str =
    "the namespace does not fit the qualified name (a prefix needs a namespace; \
xml and xmlns take only their own)";

/// [`dom_name::validate_and_extract`] with its refusals as the `*_ns` methods
/// of both representations raise them: `ArgumentError` for a name ("invalid
/// `what` name"), `Makiri::Error` for a namespace. Returns (prefix, local
/// name), the prefix empty when there is none.
pub fn dom_extract<'q>(
    ruby: &Ruby,
    ns: &[u8],
    qname: &'q [u8],
    local_ok: fn(&[u8]) -> bool,
    what: &str,
) -> Result<(&'q [u8], &'q [u8]), Error> {
    dom_name::validate_and_extract(ns, qname, local_ok).map_err(|e| match e {
        dom_name::ExtractError::Name => {
            Error::new(ruby.exception_arg_error(), format!("invalid {what} name"))
        }
        dom_name::ExtractError::Namespace => makiri_error(NS_MISFIT),
    })
}

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

/// `Document#tree_version`: an Integer that grows with every edit that can
/// change a child list of a node the document owns (attached, detached or in
/// a fragment) - add, remove, replace, `inner_html=`, an element's
/// `content=` and the like, and both documents of a move between them.
/// Attribute edits (`attribute_version`'s) and character-data edits (a Text,
/// Comment, CDATA or PI node's `content=`) leave it alone. A reader caching a child list keys it by this.
pub fn document_tree_version(rb_self: Value) -> Result<u64, magnus::Error> {
    crate::bridge::ruby::entry(|| crate::bridge::wrapper::tree_version(rb_self))
}

/// `Document#attribute_version`: an Integer that grows with every edit of an
/// attribute of an element the document owns - added, removed, its value set
/// (to the same value too), an Attr's `content=` included. Child-list edits
/// (`tree_version`'s) and character-data edits leave it alone. A reader
/// caching what depends on attributes keys it by this.
pub fn document_attribute_version(rb_self: Value) -> Result<u64, magnus::Error> {
    crate::bridge::ruby::entry(|| crate::bridge::wrapper::attribute_version(rb_self))
}
