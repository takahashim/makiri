//! The receiver cleared for an edit (`HtmlEdit`), and the factories' gate
//! (`new_node`): the two ways to the document's `&mut`.

#![forbid(unsafe_code)]

use super::*;

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// The receiver cleared for an edit (not frozen, nor an Attr whose owner
/// is), and the PROOF of it: [`edit`] is the only way to build one and
/// [`HtmlEdit::with_tree`] (or its attribute and data twins) the only way to spend
/// it, ONCE - they take `self`. The XML side's `Editing`, spent the same way.
///
/// The checks and the index drop are two steps on purpose. The checks come
/// first, so a frozen receiver is reported before a bad argument. The drop
/// comes last, once every argument is converted. (A conversion used to run the
/// argument's `#to_s`, and a query there rebuilt the indexes from the tree the
/// edit was about to change. No conversion runs Ruby code now, but a
/// conversion can still raise - a transcode, a Symbol's name - and it must not
/// do so after the indexes are gone and before the tree has changed.)
pub struct HtmlEdit<'a> {
    /// Borrowed mutably for the edit's life: a read of the receiver's
    /// document (`HtmlSelf::doc`) cannot still be held when the edit begins,
    /// nor be taken while it runs - the edit compacts the byte store those
    /// reads borrow. A COPY of the receiver is not covered; reads go through
    /// the receiver itself.
    this: &'a mut HtmlSelf,
}

/// Clear the receiver for an edit. Convert every argument after this and
/// before [`HtmlEdit::with_tree`].
pub fn edit<'a>(ruby: &Ruby, this: &'a mut HtmlSelf) -> Result<HtmlEdit<'a>, Error> {
    this.value.check_frozen()?;
    check_attr_owner_frozen(this)?;
    /* Before any argument is converted: see `account_growth`. */
    crate::bridge::wrapper::account_growth_of(ruby, &this.document);
    Ok(HtmlEdit { this })
}

/// An Attr receiver's edit changes its OWNER's attribute list, so a frozen
/// owner refuses it as it refuses `delete` - checked with the receiver's own
/// frozen flag, before and after the arguments are converted.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "an Attr's owner, as the wrapper cache's identity key; it does not escape"
    )
)]
fn check_attr_owner_frozen(this: &HtmlSelf) -> Result<(), Error> {
    let owner = {
        let view = this.read()?;
        let node = view.node();
        match (node.node_type(), node.parent()) {
            (NodeType::Attribute, Some(owner)) => Some(RawNode::from(owner)),
            _ => None,
        }
    };
    match owner {
        Some(owner) => crate::bridge::wrapper::check_node_frozen(this.document(), owner),
        None => Ok(()),
    }
}

impl<'a> HtmlEdit<'a> {
    /// The receiver's node, read-only, for a check that no argument can
    /// change (its node type).
    pub fn node_type(&self) -> Result<NodeType, Error> {
        Ok(self.this.read()?.node().node_type())
    }

    /// Lend the document for the change, with the receiver's node, and drop
    /// its indexes first. From here to the end of `f` nothing may run Ruby -
    /// every argument is already converted - so nothing can rebuild them, and
    /// nothing can read the document while `f` holds it.
    ///
    /// Both guards are checked again, as they were when an argument's `#to_s`
    /// ran between `edit` and here and could freeze the receiver; `edit`
    /// checks first so a frozen receiver is reported ahead of a bad argument. It takes the token, so a caller cannot reach for the
    /// document a second time after running Ruby.
    ///
    /// It counts as a change to a child list ([`record_edit`]); an
    /// attribute edit takes [`HtmlEdit::with_attributes`] instead, and a
    /// character-data edit [`HtmlEdit::with_data`].
    pub fn with_tree<R>(
        self,
        f: impl FnOnce(&mut HtmlDocMut<'_>, RawNode) -> R,
    ) -> Result<R, Error> {
        self.lend(EditKind::ChildList, f)
    }

    /// [`HtmlEdit::with_tree`] for an edit of ATTRIBUTES only - an element's,
    /// or an Attr node's value - which changes no child list: it counts
    /// towards the attribute version ([`record_edit`]) instead of the tree
    /// version.
    pub fn with_attributes<R>(
        self,
        f: impl FnOnce(&mut HtmlDocMut<'_>, RawNode) -> R,
    ) -> Result<R, Error> {
        self.lend(EditKind::Attributes, f)
    }

    /// [`HtmlEdit::with_tree`] for an edit of a Text, Comment, CDATA or PI
    /// node's DATA, which changes no child list and no attribute: no version
    /// counts it (see [`EditKind::CharacterData`]).
    pub fn with_data<R>(
        self,
        f: impl FnOnce(&mut HtmlDocMut<'_>, RawNode) -> R,
    ) -> Result<R, Error> {
        self.lend(EditKind::CharacterData, f)
    }

    fn lend<R>(
        self,
        kind: EditKind,
        f: impl FnOnce(&mut HtmlDocMut<'_>, RawNode) -> R,
    ) -> Result<R, Error> {
        self.this.value.check_frozen()?;
        check_attr_owner_frozen(self.this)?;
        record_edit_in(&self.this.document, kind);
        /* The receiver is not frozen - checked just now - and the guard refuses any other
         * read still held. `f` runs no Ruby (this method's contract with its
         * callers), and the guard goes with it. `edit` drops the indexes. */
        let mut write = html_write(&self.this.document)?;
        let mut doc = write.edit();
        Ok(f(&mut doc, self.this.raw()))
    }
}

/// Refuse a factory on `rb_doc` - not an HTML Document - before any argument
/// is converted, so that refusal wins over a bad argument. [`new_node`] checks
/// again.
pub fn check_owning_doc(rb_doc: &Value) -> Result<(), Error> {
    html_doc_check(*rb_doc)
}

/// Lend `rb_doc`'s document to `f` to make a DETACHED node - the factories'
/// and the copies' gate. A new node is in no tree, so no index changes and no
/// receiver can be frozen; what is refused, by the guard, is a document a read
/// still holds. `f` must run no
/// Ruby, as [`HtmlEdit::with_tree`]'s must not.
pub fn new_node<R>(
    ruby: &crate::bridge::rb::Ruby,
    rb_doc: &Value,
    f: impl FnOnce(&mut HtmlNewNodes<'_>) -> R,
) -> Result<R, Error> {
    lend_document(ruby, rb_doc, |write| f(&mut write.new_nodes()))
}

/// Check `rb_doc` can be lent - an HTML Document -
/// hand its document to `lend`, and report what the loan grew it by.
fn lend_document<R>(
    ruby: &crate::bridge::rb::Ruby,
    rb_doc: &Value,
    lend: impl FnOnce(&mut HtmlWrite<'_>) -> R,
) -> Result<R, Error> {
    let doc = HTML_DOC_TYPE.check(*rb_doc)?;
    /* The loan ends with this block, before the report below can run Ruby;
     * `lend` runs none. */
    let r = {
        let mut write = html_write(&doc)?;
        lend(&mut write)
    };
    /* A factory grows the document without an edit after it - a namespace
     * of a megabyte, a fragment imported - so it reports here, after the
     * loan: the arguments were converted to copies before, and what it made
     * is still a bare node, so a collection here can free nothing it needs. */
    crate::bridge::wrapper::account_growth_of(ruby, &doc);
    Ok(r)
}
