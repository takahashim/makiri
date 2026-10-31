//! `add_child` and its siblings: a node moved within its document, or adopted
//! from another (`Adoption`).

#![forbid(unsafe_code)]

use super::*;

/// A node copied in from another document, still to be taken out of it: the
/// second half of the move `appendChild` performs across documents (the XML
/// twin is `bridge::xml`'s `Adoption`). It holds the source's WRITE, which
/// [`insert`] takes before the copy is made, so finishing cannot be refused
/// and the write and the original it releases are one value.
struct Adoption<'a> {
    /// The source document, written: taken BEFORE the copy, so a refusal (a
    /// read of it still held) comes while there is nothing to take back,
    /// rather than after the copy is placed. The original is read through it
    /// for the copy, too.
    source: HtmlWrite<'a>,
    /// The source's Document, whose tree version the release bumps.
    src_document: Value,
    /// The original, in the source document.
    src: RawNode,
}

impl Adoption<'_> {
    /// The copy is placed: take the original out of its own document, so
    /// the whole reads as a move, and drop that document's indexes, which
    /// still list it. A fragment gives up its children (the DOM leaves a
    /// spliced one empty); anything else leaves its parent. Recorded before
    /// the change (`record_edit`). The source's write ends with this.
    fn finish(mut self) {
        record_edit(self.src_document, EditKind::ChildList);
        let mut d = self.source.edit();
        let n = d.node(self.src);
        if n.node_type() == NodeType::DocumentFragment {
            d.detach_children(self.src);
        } else if n.parent().is_some() {
            d.detach(self.src);
        }
    }
}

/// `FrozenError` when the node `key` names is a fragment and one of its
/// children is frozen: an insertion splices a fragment's children, relinking
/// each. A child with no wrapper cannot have been frozen, so only the node
/// table is asked ([`check_node_frozen`]), and an insertion of anything but
/// a fragment asks nothing.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "a fragment child, as the node table's key; it does not escape"
    )
)]
fn check_fragment_children_frozen(rb_doc: Value, key: HtmlNodeKey) -> Result<(), Error> {
    with_html_node(rb_doc, key, |n| {
        if n.node_type() != NodeType::DocumentFragment {
            return Ok(());
        }
        core::iter::successors(n.first_child(), |c| c.next())
            .try_for_each(|c| check_node_frozen(rb_doc, RawNode::from(c)))
    })?
}

/// `node.add_child(other)` and its siblings: put `rb_incoming` at `place`
/// relative to the receiver - moved within the document, or adopted from its
/// own - and hand back what is now in the tree: the argument, or for an adopted
/// node its copy.
///
/// Every rule is checked before anything changes (see
/// [`Insertion::check`], the shared `crate::dom_rules`); after that only the
/// adoption copy can fail, and it too runs before a link is touched.
pub fn insert(
    ruby: &Ruby,
    this: &mut HtmlSelf,
    rb_incoming: Value,
    place: Place,
) -> Result<Value, Error> {
    let document = this.document();
    let edit = edit(ruby, this)?;
    let (key, incoming_doc) = html_node_key(ruby, rb_incoming)?;
    /* The argument is relinked too - `place` changes its parent and siblings, and
     * an adoption removes it from its own document - so a frozen argument is a
     * frozen node being modified. The receiver check alone let it through, which
     * made `a.remove` raise and `span.add_child(a)` not, for the same effect on
     * `a`. A fragment's children are relinked too - the fragment is spliced -
     * so each is held to the same rule. */
    rb_incoming.check_frozen()?;
    check_fragment_children_frozen(incoming_doc, key)?;
    /* The argument resolved against its own Document - the checked boundary -
     * and kept as a reference: in the receiver's document it is read through
     * the edit's own borrow, never through a second one. */
    let incoming = node_ref(incoming_doc, key)?;
    let incoming_handle = doc_known(incoming_doc);
    /* From another document, the argument's own document is written too, so
     * it is taken for the edit before anything changes ([`Adoption::source`]).
     * Nothing between here and the release runs Ruby. */
    let adoption = match crate::bridge::ruby::same_value(incoming_doc, document) {
        true => None,
        false => Some(Adoption {
            source: html_write(&incoming_handle)?,
            src_document: incoming_doc,
            src: incoming,
        }),
    };
    let placed = edit.with_tree(|d, target| -> Result<RawNode, Error> {
        let node = match &adoption {
            None => {
                let insertion = Insertion::new(d.node(target), place, d.node(incoming))
                    .map_err(crate::bridge::dom_error::pre_insert_error)?;
                insertion
                    .check()
                    .map_err(crate::bridge::dom_error::pre_insert_error)?;
                if insertion.unchanged() {
                    return Ok(incoming);
                }
                if d.node(incoming).parent().is_some() {
                    d.detach(incoming);
                }
                incoming
            }
            Some(a) => {
                /* From another document: checked, then copied into this one -
                 * the original is released once the copy is in. */
                let src = a
                    .source
                    .parsed()
                    .resolve(key)
                    .map_err(|ForeignNode| foreign_node_error())?;
                Insertion::new(d.node(target), place, src)
                    .and_then(|i| i.check())
                    .map_err(crate::bridge::dom_error::pre_insert_error)?;
                d.import(src, true)
                    .map_err(|_| makiri_error("failed to import node"))?
            }
        };
        d.place(target, node, place);
        Ok(node)
    })??;
    let Some(adoption) = adoption else {
        return Ok(rb_incoming);
    };
    adoption.finish();
    /* The document that grew is the receiver's, by the copy; the original's
     * only lost a link. */
    crate::bridge::wrapper::account_growth(ruby, document);
    wrap_html_node(ruby, placed, document)
}
