//! The child lists `#child_count` / `#child_at` and their element twins read,
//! over a Document's child-position memo.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, Value};

use super::*;
use crate::child_index::{ChildList, ChildPositionMemo, Children, TokenTree};

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
    let doc = doc_known(rb_doc);
    let d = doc_data_known(&doc);
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
    /// The child a token names, as its Ruby wrapper; None is nil. A token,
    /// not a node: it is called once the read the token came from is over,
    /// and a node handle does not outlive that read.
    fn wrap_child(&self, ruby: &Ruby, token: Option<usize>) -> Result<Option<Value>, Error>;

    /// How many children are in `list`.
    fn child_count(&self, list: ChildList) -> Result<usize, Error> {
        with_child_memo(self.document(), |memo, version| {
            self.with_children(list, |c| c.count(memo, version))
        })?
    }

    /// The wrapped child at `index` in `list`, or nil past the end.
    fn child_at(&self, ruby: &Ruby, list: ChildList, index: usize) -> Result<Option<Value>, Error> {
        let found = self.with_children(list, |c| {
            let found =
                with_child_memo(self.document(), |memo, version| c.at(memo, version, index))?;
            Ok::<_, Error>(found.map(|n| c.tree.token(n)))
        })??;
        self.wrap_child(ruby, found)
    }
}
