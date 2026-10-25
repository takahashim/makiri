//! A Document's edit versions and its child-position memo.

#![forbid(unsafe_code)]

use crate::bridge::rb::{Error, Value};

use super::*;
use crate::child_index::{ChildList, ChildPositionMemo, Children, TokenTree};

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
/// - it is recorded once the edit's refusals are past - the frozen check, the
///   evaluation guard - and its arguments converted, immediately BEFORE the
///   change, with the representation's indexes dropped at the same point;
/// - no Ruby runs between the record and the change, so nothing can read the
///   tree in between and cache it under the new number.
///
/// Recording after the change instead would miss an edit that panics, or
/// fails having changed something, and leave a reader trusting a stale cache.
pub fn record_edit(rb_doc: Value, kind: EditKind) {
    doc_data_known(&rb_doc).update_edits(|e| e.record(kind));
}

/// `Document#tree_version`: how many child-list edits the document has seen.
/// `TypeError` for a non-Document.
pub fn tree_version(rb_doc: Value) -> Result<u64, Error> {
    Ok(DOC_TYPE.get(&rb_doc)?.edits.get().tree_version)
}

/// `Document#attribute_version`: how many attribute edits the document has
/// seen. `TypeError` for a non-Document.
pub fn attribute_version(rb_doc: Value) -> Result<u64, Error> {
    Ok(DOC_TYPE.get(&rb_doc)?.edits.get().attribute_version)
}

/* ---- the child-position memo ---- */

/// Run `f` over the document's [`ChildPositionMemo`] and its current
/// `tree_version`, which every use of the memo must pass it - the memo keeps
/// its own invariant from that (`crate::child_index`).
///
/// `f` runs inside the document's borrow, so it must not run Ruby; a walk of
/// the child links does not.
///
/// The memo's borrow spans `f` only, which walks links and runs no Ruby, so
/// no second use of it can start inside; one found borrowed is a broken
/// invariant, reported as `Makiri::InternalError`.
fn with_child_memo<R>(
    rb_doc: Value,
    f: impl FnOnce(&mut ChildPositionMemo, u64) -> R,
) -> Result<R, Error> {
    let d = doc_data_known(&rb_doc);
    let version = d.edits.get().tree_version;
    let mut memo = d.child_memo.try_borrow_mut().map_err(|_| {
        crate::bridge::ruby::internal_error("a Document's child memo was found in use")
    })?;
    Ok(f(&mut memo, version))
}

/// A receiver whose child lists `#child_count` / `#child_at` and their
/// element twins read - an HTML or XML node or Document. A representation
/// says which tree and node it is and how a child is wrapped; counting and
/// indexing are written once, here, and the four Ruby methods once, over
/// this, in `glue::node`.
pub trait ChildListHost: crate::bridge::rb::TryConvert {
    type Tree<'a>: TokenTree
    where
        Self: 'a;

    /// The Document whose memo the lists use.
    fn document(&self) -> Value;
    /// Run `f` over the receiver's `list`. The tree is made for the call and
    /// not kept: an HTML arena tree borrows its document, which an edit later
    /// in the same method takes by `&mut`. `f` runs under the document's read,
    /// so it must not run Ruby, and makes nothing Ruby could raise from.
    fn with_children<R>(
        &self,
        list: ChildList,
        f: impl for<'a> FnOnce(Children<'a, Self::Tree<'a>>) -> R,
    ) -> Result<R, Error>;
    /// The child a token names, as its Ruby wrapper; None is nil. Called
    /// once the read the token came from is over, since wrapping allocates.
    fn wrap_child(&self, token: Option<usize>) -> Result<Option<Value>, Error>;

    /// How many children are in `list`.
    fn child_count(&self, list: ChildList) -> Result<usize, Error> {
        with_child_memo(self.document(), |memo, version| {
            self.with_children(list, |c| c.count(memo, version))
        })?
    }

    /// The wrapped child at `index` in `list`, or nil past the end.
    fn child_at(&self, list: ChildList, index: usize) -> Result<Option<Value>, Error> {
        let found = self.with_children(list, |c| {
            let found =
                with_child_memo(self.document(), |memo, version| c.at(memo, version, index))?;
            Ok::<_, Error>(found.map(|n| c.tree.token(n)))
        })??;
        self.wrap_child(found)
    }
}
