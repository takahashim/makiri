//! Reading a stored [`NodeWord`] back as a typed Ruby node.
//!
//! The one place a NodeSet's or query result's word becomes a wrapper, which
//! the document's [`DocKind`] is what justifies - so `wrapper`, the layer that
//! only stores, does not have to know the two front doors that build wrappers.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, Value};

use crate::bridge::wrapper::{DocKind, NodeWord};

/// Wrap `node`, a node of `document`, under the representation `kind` names -
/// the one place a stored [`NodeWord`] (a NodeSet's, a query result's) is read
/// back as a typed node, which `kind` is what justifies.
///
/// Checked: a word that names no node of `document` is `InternalError` - a
/// broken invariant of the set or the query that stored it, never a wrapper
/// for another document's node. (This was an `unsafe fn` once, its contract
/// that the word is the document's; the arena's checked lookup makes that a
/// test rather than a promise, for a stamp compare and a bounds check.)
pub(in crate::bridge) fn wrap_doc_node(
    ruby: &Ruby,
    kind: DocKind,
    node: NodeWord,
    document: Value,
) -> Result<Value, Error> {
    match kind {
        DocKind::Xml => match node.xml() {
            Some(id) => crate::bridge::xml::wrap_xml_node_checked(ruby, id, document),
            None => Ok(crate::bridge::ruby::nil(ruby)),
        },
        DocKind::Html => match node.html() {
            Some(n) => crate::bridge::html::wrap_html_node_checked(ruby, n, document),
            None => Ok(crate::bridge::ruby::nil(ruby)),
        },
    }
}
