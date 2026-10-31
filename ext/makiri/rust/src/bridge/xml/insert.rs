//! Two arenas at once: `add_child` and its siblings adopting from another
//! document (`Adoption`), and `Document#import_node`'s copy.

#![forbid(unsafe_code)]

use super::*;

/* ------------------------------------------------------------------ */
/* two arenas at once: adopting and importing                         */
/* ------------------------------------------------------------------ */
/* The node methods live in `glue::xml_node::mutate`. What stays here is what
 * holds two arenas - an XML one and another, XML or HTML - at the same time,
 * which `with_arena_for_new_node`'s one-at-a-time lending cannot express. */

/// A node copied in from another document, still to be taken out of it: the
/// second half of the move `appendChild` performs across arenas. It holds the
/// source's WRITE, which [`insert`] takes before the copy is made, so
/// finishing cannot fail and cannot be skipped: there is nothing left to look
/// up or to be refused, and the write and the copy are one value rather than
/// two that must agree.
struct Adoption<'a> {
    /// The source's arena, written: the original is taken out of it, and its
    /// name index goes with it. Taken BEFORE the copy, so a refusal (a read
    /// of the source still held) comes while there is nothing to take back,
    /// rather than after the copy is in the target arena.
    sdoc: XmlWrite<'a>,
    /// The source's Document, whose tree version the removal bumps.
    src_document: Value,
    src: NodeId,
    /// The target arena's high-water mark from before the copy, so a copy
    /// the placement refuses is taken back out rather than left in the
    /// arena, holding its bytes and node budget, for every refused call.
    mark: crate::xml::arena::Mark,
    /* No field keeps the source's Document: the node is the method's
     * argument, which Ruby's stack keeps, and the node's slot keeps its
     * Document. (A `Value` field that is never read kept nothing - the
     * optimizer may drop it.) */
}

impl Adoption<'_> {
    /// The placement refused the copy: take it back out of `target`, the
    /// arena it was imported into. A refused placement wrote nothing (`place`
    /// validates and resolves before it links), so the copy is still
    /// unlinked and unwrapped, and goes as a failed copy does
    /// (`Document::rewind`); the original stays where it was.
    fn undo_copy(&self, target: &mut XmlDoc) {
        target.rewind(self.mark);
    }

    /// The placement took the copy: empty the original out of its old
    /// document, whose name index goes with it. The source's write ends
    /// with this.
    fn finish(mut self) {
        let sdoc = &mut *self.sdoc;
        /* Invalidated and recorded before the removal (`record_edit`). */
        sdoc.invalidate_name_index();
        record_edit(self.src_document, EditKind::ChildList);
        if sdoc.type_(self.src) == Some(ArenaKind::DocumentFragment) {
            while let Some(c) = sdoc.first_child(self.src) {
                remove_node(sdoc, c);
            }
        } else {
            remove_node(sdoc, self.src);
        }
    }
}

/// `node.add_child(arg)` and its siblings: put `arg` at `at` relative to the
/// receiver - moved within the document, or adopted from its own - and hand
/// back what is now in the tree: the argument, or for an adopted node its copy.
/// The HTML twin is `bridge::html::insert`.
///
/// The whole edit is the bridge's: the receiver cleared for editing, the
/// argument resolved (and copied, when it is another document's), every rule
/// checked and the namespaces resolved by the placing, and - only once that
/// has succeeded - the adopted original taken out of its own document
/// ([`Adoption::finish`]), which nothing outside this module can call.
pub fn insert(
    ruby: &Ruby,
    mut this: XmlSelf,
    arg: Value,
    at: crate::xml::mutate::Place,
) -> Result<Value, Error> {
    let edit = begin_edit(ruby, &mut this)?;
    let target = edit.document;
    let target_doc = target.value();
    let (src, src_document) = incoming_ref(ruby, arg)?;
    /* The source's handle, which its write in an adoption borrows. */
    let source;
    let (node, adoption) = if crate::bridge::ruby::same_value(src_document, target_doc) {
        /* The same arena: a move. */
        (src, None)
    } else {
        /* An adoption writes the source too, so it is taken for the edit
         * BEFORE the copy is made ([`Adoption::sdoc`]). Nothing between here
         * and the release runs Ruby. */
        source = doc_known(src_document);
        let sdoc = xml_write(&source)?;
        let (mark, copied) = {
            /* Two distinct arenas (the documents differ): the target
             * written, the source read through its own write. */
            let mut dst = xml_write(target)?;
            (dst.mark(), import_subtree(&mut dst, &sdoc, src))
        };
        let copy = xml_mut_result(copied)?;
        (
            copy,
            Some(Adoption {
                sdoc,
                src_document,
                src,
                mark,
            }),
        )
    };
    xml_mut_result(edit.with_insertion(node, |d, target| {
        let placed = crate::xml::mutate::place(d, target, node, at);
        if let (Err(_), Some(a)) = (&placed, &adoption) {
            a.undo_copy(d);
        }
        placed
    })?)?;
    if let Some(a) = adoption {
        a.finish(); /* the source's write ends with it */
        /* The receiver's document grew by the copy. */
        crate::bridge::wrapper::account_growth(ruby, target_doc);
    }
    wrap_xml_node(ruby, node, this.document())
}

/// `arg` as the node an insertion takes in, and the Document it lives in: a
/// node of an XML document, not frozen. The copy of one from another document
/// is [`insert`]'s to make, once it holds that document's write.
fn incoming_ref(ruby: &Ruby, arg: Value) -> Result<(NodeId, Value), Error> {
    if !is_kind_of(ruby, arg, &CLASS_NODE) {
        return Err(crate::bridge::ruby::type_error(
            "expected a Makiri::XML node",
        ));
    }
    /* One read of the wrapper, as a receiver's: `TypeError` for an HTML node
     * or Document, as `xml_node_unwrap` raises. */
    let incoming = <XmlSelf as crate::bridge::rb::TryConvert>::try_convert(ruby, arg)?;
    let (id, document) = (incoming.id, incoming.document());
    /* Inserting `arg` relinks IT - its parent, prev and next all change, and an
     * adoption takes it out of its own document - so a frozen argument is a
     * frozen node being modified, which the receiver check alone let through:
     * `a.remove` raised and `b.add_child(a)` did not, for the same effect on `a`.
     *
     * A fragment argument splices its children, relinking each, so each is
     * held to the same rule; a child with no wrapper cannot have been frozen,
     * so only the node table is asked (`check_node_frozen`). */
    arg.check_frozen()?;
    let handle = doc_known(document);
    let doc = xml_read(&handle)?;
    if doc.type_(id) == Some(ArenaKind::DocumentFragment) {
        core::iter::successors(doc.first_child(id), |&c| doc.next(c))
            .try_for_each(|c| crate::bridge::wrapper::check_node_frozen(document, c))?;
    }
    Ok((id, document))
}

/// `Document#import_node`'s copy: `node_v` - XML from any document, or HTML -
/// copied, detached, into the XML Document `rb_self`. The source is untouched.
pub fn import_copy(
    ruby: &Ruby,
    rb_self: Value,
    node_v: Value,
    deep: bool,
) -> Result<NodeId, Error> {
    xml_doc_check(rb_self)?; /* TypeError for anything but an XML Document */
    let copy = match node_repr(node_v) {
        NodeRepr::Xml => {
            /* Read, not written: the copy goes into the receiver's arena. */
            let src_document = xml_node_document(ruby, node_v)?;
            let src = xml_node_unwrap(ruby, node_v)?;
            let copied = if crate::bridge::ruby::same_value(src_document, rb_self) {
                /* Same arena: the single-`&mut` clone path. */
                clone_node(&mut *xml_write(&doc_known(rb_self))?, src, deep)
            } else {
                let (target, source) = (doc_known(rb_self), doc_known(src_document));
                let mut dst = xml_write(&target)?;
                let read = xml_read(&source)?;
                copy_node_from(&mut dst, &read, src, deep)
            };
            xml_mut_result(copied)?
        }
        NodeRepr::Html => {
            /* The HTML source, read for the closure; the copy goes into the
             * receiver's arena, another document. */
            let copied = with_arg_node(ruby, node_v, |src| {
                Ok::<_, Error>(cross_html_to_xml(
                    &mut *xml_write(&doc_known(rb_self))?,
                    src,
                    deep,
                ))
            })??;
            xml_mut_result(copied)?
        }
        NodeRepr::Other => {
            return Err(crate::bridge::ruby::type_error(
                "import_node expects a Makiri node",
            ))
        }
    };
    /* A copy grows the arena with no edit after it: reported after the loan
     * (it may run a collection; the copy is a bare node of the arena). */
    crate::bridge::wrapper::account_growth(ruby, rb_self);
    Ok(copy)
}
