//! The node table: one Ruby wrapper per node of a Document, kept in the
//! Document's slot 0, and what is asked of it - the wrapper for a node, made
//! once and handed out ever after, and whether that wrapper is frozen.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, Value};
use crate::bridge::ruby::internal_error;
use crate::bridge::typed::Birth;
use crate::node_type::NodeType;

use super::*;

/// The one wrapper for `node`, a node of `document`: the cached one, or a
/// fresh one of its leaf class, then cached. A DOCUMENT node maps back onto
/// the Ruby Document rather than getting a second wrapper. The one wrap of
/// both representations ([`NodeHandleSource`] supplies what differs).
///
/// The table is asked first, so a node wrapped before costs no read of the
/// document. A node `document` does not have - another document's, or a
/// stale word - is `InternalError`, never a wrapper.
///
/// One wrapper per node: navigating to a node twice must give the SAME object,
/// or everything that lives on a Ruby object is silently lost - `equal?`, an
/// instance variable, a singleton method, `freeze`. A Document is already its
/// own wrapper, which is why it needs no entry. So a wrapper is handed out
/// only once it is in the table: `Err` (`NoMemoryError`) when the entry
/// cannot be made, and the fresh wrapper, which nothing refers to, is left
/// for the GC - never an object outside the table.
pub(in crate::bridge) fn wrap_dom_node<N: NodeHandleSource>(
    ruby: &Ruby,
    node: N,
    document: Value,
) -> Result<Value, Error> {
    wrap_dom_node_typed(ruby, node, None, document)
}

/// [`wrap_dom_node`] for a caller that read the node's type already, in the read
/// it found the node in - one read the fewer on a miss.
pub(in crate::bridge) fn wrap_dom_node_of_type<N: NodeHandleSource>(
    ruby: &Ruby,
    node: N,
    node_type: NodeType,
    document: Value,
) -> Result<Value, Error> {
    wrap_dom_node_typed(ruby, node, Some(node_type), document)
}

fn wrap_dom_node_typed<N: NodeHandleSource>(
    ruby: &Ruby,
    node: N,
    known: Option<NodeType>,
    document: Value,
) -> Result<Value, Error> {
    let Some((table, key)) = table_and_key(document, &node)? else {
        return Err(internal_error("a node of another document"));
    };
    if let Some(cached) = table.get(key)? {
        return Ok(cached);
    }
    let node_type = match known {
        Some(t) => t,
        None => node
            .node_type(document)?
            .ok_or_else(|| internal_error("a node its document does not have"))?,
    };
    if node_type == NodeType::Document {
        return Ok(document);
    }
    let klass = N::classes().class_for(ruby, node_type)?;
    let fresh = N::wrapper_type().wrap(
        ruby,
        Birth {
            klass,
            data: NodeData {
                node: node.into_handle(document),
            },
            roots: [document],
        },
    )?;
    /* The table entry: a Ruby reference, which the GC marks and moves
     * itself. A failure leaves `fresh` unseen and unreferenced. */
    table.set(key, fresh)?;
    Ok(fresh)
}

/// `document`'s node table and `source`'s key in it - its index - or `None`
/// for a node of another document, whose index names another node here.
/// One type check reads both.
#[inline]
fn table_and_key(
    document: Value,
    source: &impl NodeHandleSource,
) -> Result<Option<(crate::nupur::table::IntTable, u32)>, Error> {
    let (index, stamp) = source.index_and_stamp();
    let doc = DOC_TYPE.check(document)?;
    if doc.data().stamp() != stamp {
        return Ok(None);
    }
    let table = crate::nupur::table::IntTable::from_value(doc.root::<0>())
        .ok_or_else(|| internal_error("a Document without its node table"))?;
    Ok(Some((table, index)))
}

/// The one wrapper for `source` under `rb_doc`, or None the first time -
/// and for a node of another document, which has none here.
pub(in crate::bridge) fn cached_node(
    rb_doc: Value,
    source: &impl NodeHandleSource,
) -> Result<Option<Value>, Error> {
    match table_and_key(rb_doc, source)? {
        Some((table, key)) => table.get(key),
        None => Ok(None),
    }
}

/// `FrozenError` when the node `source` names has a wrapper and that wrapper is
/// frozen. A node with no wrapper yet cannot have been frozen. For an edit
/// that reaches a node other than its receiver - an Attr's owner element,
/// whose attribute list an Attr's `remove` or `content=` changes.
pub(in crate::bridge) fn check_node_frozen(
    rb_doc: Value,
    source: impl NodeHandleSource,
) -> Result<(), Error> {
    match cached_node(rb_doc, &source)? {
        Some(wrapper) => crate::bridge::rb::ReprValue::check_frozen(wrapper),
        None => Ok(()),
    }
}

/// [`check_node_frozen`] of an Attr's owner element, when the edited node is
/// an Attr (`owner`: `None` otherwise): an Attr's edit changes its owner's
/// attribute list, so a frozen owner refuses it as it refuses `delete`. How
/// the owner is read is each DOM's; this is the rest, once.
pub(in crate::bridge) fn check_attr_owner_frozen(
    rb_doc: Value,
    owner: Option<impl NodeHandleSource>,
) -> Result<(), Error> {
    match owner {
        Some(owner) => check_node_frozen(rb_doc, owner),
        None => Ok(()),
    }
}
