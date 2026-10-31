//! The node table: one Ruby wrapper per node of a Document, kept in the
//! Document's slot 0, and what is asked of it - the wrapper for a node, made
//! once and handed out ever after, and whether that wrapper is frozen.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, Value};
use crate::bridge::ruby::internal_error;

use super::*;

/// The one wrapper of class `klass` (a `ty` object) for a node under
/// `document`: the cached one, or a fresh one built from `source` and then
/// cached. The shared half of the two `wrap_*_node` functions.
///
/// The candidate supplies both its cache identity and, on a miss, its stored
/// handle. HTML key minting is therefore skipped on a cache hit, while the
/// token and handle cannot come from different nodes.
///
/// One wrapper per node: navigating to a node twice must give the SAME object,
/// or everything that lives on a Ruby object is silently lost - `equal?`, an
/// instance variable, a singleton method, `freeze`. A Document is already its
/// own wrapper, which is why it needs no entry. So a wrapper is handed out
/// only once it is in the table: `Err` (`NoMemoryError`) when the entry
/// cannot be made, and the fresh wrapper, which nothing refers to, is left
/// for the GC - never an object outside the table.
///
/// The caller vouches that the node is a node of `document` of the
/// representation `ty` wraps, and `klass` a class of it: only the two
/// `wrap_*_node` functions call this, each for its own kind.
pub(in crate::bridge) fn wrap_cached(
    ruby: &Ruby,
    ty: &'static NodeWrapperType,
    klass: crate::bridge::rb::DataClass,
    source: impl NodeHandleSource,
    document: Value,
) -> Result<Value, Error> {
    let Some((table, key)) = table_and_key(document, &source)? else {
        return Err(internal_error("a node of another document"));
    };
    if let Some(cached) = table.get(key)? {
        return Ok(cached);
    }
    let node = source.into_handle(document);
    let fresh = ty.wrap(ruby, klass, NodeData { node }, [document])?;
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
