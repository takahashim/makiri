//! Reading a stored [`NodeWord`] back as a typed Ruby node.
//!
//! The one place a NodeSet's or query result's word becomes a wrapper, which
//! the document's [`DocKind`] is what justifies - so `wrapper`, the layer that
//! only stores, does not have to know the two front doors that build wrappers.

#![allow(unsafe_code)]

use magnus::Value;

use crate::bridge::wrapper::{DocKind, NodeWord};

/// Wrap `node`, a node of `document`, under the representation `kind` names -
/// the one place a stored [`NodeWord`] (a NodeSet's, a query result's) is read
/// back as a typed node, which `kind` is what justifies.
///
/// # Safety
/// `node` is a live node of `document`, and `document` is of `kind`.
pub(in crate::bridge) unsafe fn wrap_doc_node(
    kind: DocKind,
    node: NodeWord,
    document: Value,
) -> Value {
    match kind {
        DocKind::Xml => match node.xml() {
            Some(id) => crate::bridge::xml::wrap_xml_node(id, document),
            None => crate::bridge::ruby::nil(),
        },
        DocKind::Html => match node.html() {
            Some(n) => crate::bridge::html::wrap_html_node(n, document),
            None => crate::bridge::ruby::nil(),
        },
    }
}
