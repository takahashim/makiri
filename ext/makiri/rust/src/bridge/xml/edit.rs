//! The receiver cleared for an edit (`Editing`), and the factories' gate
//! (`with_arena_for_new_node`): the two ways to an arena's `&mut`.

#![forbid(unsafe_code)]

use super::*;

/* ------------------------------------------------------------------ */
/* lending the arena for an edit                                      */
/* ------------------------------------------------------------------ */

/// Run `f` on `document`'s arena to build a DETACHED node - the factories'
/// gate, and the one way a caller outside this module gets `&mut` to an XML
/// arena without a receiver.
///
/// A detached node is not in the tree, so it is not in the name index and there
/// is no receiver to check for frozenness. A TREE EDIT is [`Editing`]: it needs
/// both, and the only way to get the `&mut` for one is to spend the token
/// `begin_edit` hands out. The name says which of the two this is, because a
/// mutator reaching for "the arena, mutably" would otherwise skip the checks
/// simply by asking for the wrong thing - which is how the XML side came to have
/// nine sites that correctly skip the index invalidation and eight that must
/// not, with nothing but a reader's judgement telling them apart.
///
/// Refused while a read of the document is held (`wrapper::xml_write`). The
/// `&mut` lives for `f` alone, and `f` must not run Ruby:
/// the arena is `Vec`s, so Ruby code that read or wrote this same document
/// meanwhile would alias the borrow. That is why a method converts and checks
/// its arguments FIRST and only then calls this, with nothing but engine calls
/// inside - which are Ruby-free by construction.
pub fn with_arena_for_new_node<R>(
    document: Value,
    f: impl FnOnce(&mut XmlDoc) -> R,
) -> Result<R, Error> {
    let document = doc_known(document);
    let mut write = xml_write(&document)?;
    Ok(f(&mut write))
}

/// The receiver cleared for an edit, and the PROOF of it.
///
/// [`begin_edit`] is the only way to build one and [`Editing::with_arena`] (or
/// its attribute and data twins) the only way to spend it, so a tree edit
/// cannot reach the arena without the two things that must happen first: the
/// frozen check, and dropping the name index the edit is about to invalidate.
///
/// Spent ONCE - the three take `self` - as `HtmlEdit` is: one permit, one
/// edit, one record of it. Each spending checks again, so a second would not
/// be unsound; taking the permit narrows what a caller can get wrong.
///
/// [`with_arena_for_new_node`] stays for the FACTORIES, which build a detached node -
/// not in the tree, so not in the index, and with no receiver to freeze. The
/// HTML side has the same pair, `edit` and `HtmlEdit::with_tree`; XML had the `&mut`
/// gate and the invalidate gate as two separate calls, and whether a site needed
/// the second was a judgement the reader had to make at each of seventeen.
pub struct Editing<'a> {
    /// The receiver, frozen-checked again at the change.
    receiver: Value,
    /// The receiver's Document, borrowed from the receiver, which
    /// [`begin_edit`] took mutably for the edit's life: a read of its
    /// document (`XmlSelf::read`) cannot still be held when the edit
    /// begins, nor be taken while it runs - the arena it borrows is the one
    /// the edit may grow. A COPY of the receiver is not covered; reads go
    /// through the receiver itself.
    pub(super) document: &'a DocHandle,
    id: NodeId,
}

impl Editing<'_> {
    /// The node being edited.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// The arena, read while the edit is held and before it is spent: the
    /// read borrows the permit, so it is over before the change can run.
    pub fn read(&self) -> Result<XmlRead<'_>, Error> {
        xml_read(self.document)
    }

    /// Its document, for the Ruby-side work an edit does around the arena call.
    pub fn document(&self) -> Value {
        self.document.value()
    }

    /// Lend the arena for the change. `f` runs no Ruby (see [`with_arena_for_new_node`]),
    /// so every argument is converted BEFORE this. The frozen check is in
    /// `begin_edit`, ahead of that conversion, so a frozen receiver is reported
    /// before a bad argument, and again here. (The second check dates from
    /// when a conversion ran the argument's `#to_s`, which could freeze the
    /// receiver; no conversion runs Ruby code now.)
    ///
    /// It is also where the name index is dropped, just before `f` changes what
    /// it indexes, so nothing between `begin_edit` and here - a conversion's
    /// allocation included - can see the index gone before the tree changes.
    ///
    /// It counts as a change to a child list ([`record_edit`]); an
    /// attribute edit takes [`Editing::with_attributes`] instead, and a
    /// character-data edit [`Editing::with_data`].
    pub fn with_arena<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::ChildList, f)
    }

    /// [`Editing::with_arena`] for an insertion, which also counts as an
    /// attribute edit when placing the subtree may decide an attribute's
    /// namespace ([`crate::xml::mutate::decides_attr_ns`]): an attribute set
    /// with a prefix unbound on a detached element reads a new
    /// `namespace_uri` once its element is placed under a declaration.
    pub(super) fn with_insertion<R>(
        self,
        node: NodeId,
        f: impl FnOnce(&mut XmlDoc, NodeId) -> R,
    ) -> Result<R, Error> {
        let attrs = crate::xml::mutate::decides_attr_ns(&*xml_read(self.document)?, node);
        let also = attrs.then_some(EditKind::Attributes);
        self.lend_recording(EditKind::ChildList, also, f)
    }

    /// [`Editing::with_arena`] for an edit of ATTRIBUTES only - an element's,
    /// or an Attr node's value - which changes no child list: it counts
    /// towards the attribute version ([`record_edit`]) instead of
    /// the tree version.
    pub fn with_attributes<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::Attributes, f)
    }

    /// [`Editing::with_arena`] for an edit of a Text, Comment, CDATA or PI
    /// node's DATA, which changes no child list and no attribute: no version
    /// counts it (see [`EditKind::CharacterData`]).
    pub fn with_data<R>(self, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend(EditKind::CharacterData, f)
    }

    fn lend<R>(self, kind: EditKind, f: impl FnOnce(&mut XmlDoc, NodeId) -> R) -> Result<R, Error> {
        self.lend_recording(kind, None, f)
    }

    /// [`Editing::lend`], recording `also` beside `kind` when the edit is
    /// both.
    fn lend_recording<R>(
        self,
        kind: EditKind,
        also: Option<EditKind>,
        f: impl FnOnce(&mut XmlDoc, NodeId) -> R,
    ) -> Result<R, Error> {
        let id = self.id;
        self.receiver.check_frozen()?;
        check_attr_owner_frozen(self.document, id)?;
        /* Recorded once both refusals are past and BEFORE the change, with no
         * Ruby run in between (`record_edit`): an edit that then fails, or a
         * panic in it, still invalidates what it may have changed. */
        record_edit_in(self.document, kind);
        if let Some(also) = also {
            record_edit_in(self.document, also);
        }
        let mut write = xml_write(self.document)?;
        write.invalidate_name_index();
        Ok(f(&mut write, id))
    }
}

/// An Attr receiver's edit changes its OWNER's attribute list, so a frozen
/// owner refuses it as it refuses `delete` - checked with the receiver's own
/// frozen flag, before and after the arguments are converted.
fn check_attr_owner_frozen(document: &DocHandle, id: NodeId) -> Result<(), Error> {
    let owner = {
        let doc = xml_read(document)?;
        match (doc.type_(id), doc.parent(id)) {
            (Some(ArenaKind::Attribute), Some(owner)) => Some(owner),
            _ => None,
        }
    };
    match owner {
        Some(owner) => crate::bridge::wrapper::check_node_frozen(document.value(), owner),
        None => Ok(()),
    }
}

/// The receiver cleared for an edit - not frozen, nor an Attr whose owner is.
/// The name index is dropped later, by [`Editing::with_arena`].
pub fn begin_edit<'a>(ruby: &Ruby, this: &'a mut XmlSelf) -> Result<Editing<'a>, Error> {
    this.value.check_frozen()?;
    check_attr_owner_frozen(&this.document, this.id)?;
    /* Before any argument is converted: see `account_growth`. */
    crate::bridge::wrapper::account_growth_of(ruby, &this.document);
    /* The `&mut` is given up for a shared borrow of the same life, so the
     * caller's receiver stays borrowed until the edit is spent. */
    let this: &'a XmlSelf = this;
    Ok(Editing {
        receiver: this.value,
        document: &this.document,
        id: this.id,
    })
}
