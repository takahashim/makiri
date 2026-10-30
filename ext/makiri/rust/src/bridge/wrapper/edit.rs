//! A Document's edit versions: what an edit changed, as the keys readers'
//! caches are invalidated by.

#![forbid(unsafe_code)]

use crate::bridge::rb::{Error, Value};

use super::*;

/* ---- the edit versions ---- */

/// What an edit changes, and so which version counts it ([`record_edit`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    /// A child list of a node the document owns - attached, detached or inside
    /// a fragment alike: `Document#tree_version`.
    ChildList,
    /// An attribute of an element the document owns - added, removed, its
    /// value set (to the same value too) - or an Attr's value:
    /// `Document#attribute_version`.
    Attributes,
    /// A Text, Comment, CDATA or PI node's data. No version counts it: no
    /// reader has asked for one, and `tree_version` counting it made every
    /// keystroke in a text field refill every child-list cache.
    CharacterData,
}

/// Record an edit of `kind` to a node `rb_doc` owns: the one way a version
/// moves. Called where an edit is handed its mutable node (`HtmlEdit`,
/// `Editing`) and for the source document of an adoption, so no mutator can
/// miss it. Dropping what the edit invalidates is the representation's own
/// step, taken beside this one: an HTML document's indexes (dropped by the
/// parse handle's `edit`), the XML arena's name index (the arena's).
///
/// The versions are INVALIDATION keys, not edit counts. The contract, for HTML
/// and XML alike:
///
/// - an unchanged version means the data it keys did not change -
///   `tree_version` the child lists, `attribute_version` the attributes;
/// - a changed one may be conservative: an edit that then fails, or stops
///   part-way, still moved it;
/// - it is recorded once the edit's refusals are past - the frozen checks -
///   and its arguments converted, immediately BEFORE the
///   change, with the representation's indexes dropped at the same point;
/// - no Ruby runs between the record and the change, so nothing can read the
///   tree in between and cache it under the new number.
///
/// Recording after the change instead would miss an edit that panics, or
/// fails having changed something, and leave a reader trusting a stale cache.
pub fn record_edit(rb_doc: Value, kind: EditKind) {
    record_edit_in(&doc_known(rb_doc), kind);
}

/// [`record_edit`] for a caller that already holds the Document's handle.
pub fn record_edit_in(doc: &DocHandle, kind: EditKind) {
    doc_data_known(doc).update_edits(|e| e.record(kind));
}

/// `Document#tree_version`: how many child-list edits the document has seen.
/// `TypeError` for a non-Document.
pub fn tree_version(rb_doc: Value) -> Result<u64, Error> {
    Ok(DOC_TYPE.check(rb_doc)?.data().edits.get().tree_version)
}

/// `Document#attribute_version`: how many attribute edits the document has
/// seen. `TypeError` for a non-Document.
pub fn attribute_version(rb_doc: Value) -> Result<u64, Error> {
    Ok(DOC_TYPE.check(rb_doc)?.data().edits.get().attribute_version)
}
