//! A method's HTML receiver (`HtmlSelf`) and its document read (`HtmlView`): what
//! the readers read through, the receiver's text and source line, its child
//! lists, and the checked way to a node argument (`with_arg_node`).

#![forbid(unsafe_code)]

use super::*;

/* ---- the document's own bytes and text ---- */

/// The descendant text of `node`, which `read` lends, as one Ruby String, by
/// walking: one walk collects the text slices, and one pre-sized copy joins
/// them. Out of memory for the list, the walk runs twice instead - once to
/// size, once to copy. A walk of the document's tree is counted toward
/// building the text index (`HtmlParsed::note_text_walk`); one outside it (a
/// fragment, a detached subtree), which the index would not serve, is not.
///
/// Made with the read and the slice list held; a failed allocation is `Err`,
/// and both drop.
fn subtree_text(ruby: &Ruby, read: &HtmlRead<'_>, node: HtmlNode<'_>) -> Result<Value, Error> {
    use falloc::VecGrow;
    let texts = || {
        core::iter::successors(node.first_child(), move |c| c.preorder_next(node))
            .filter_map(|c| c.char_data())
            .filter(|d| !d.is_empty())
    };
    let mut slices: Vec<&[u8]> = Vec::new();
    let mut walked = 0usize;
    let collected = core::iter::successors(node.first_child(), |c| c.preorder_next(node))
        .inspect(|_| walked += 1)
        .filter_map(|c| c.char_data())
        .filter(|d| !d.is_empty())
        .try_for_each(|d| slices.falloc_push(d))
        .is_ok();
    let in_tree = node.ancestors().last().unwrap_or(node).node_type() == NodeType::Document;
    if collected && in_tree {
        read.parsed().note_text_walk(walked);
    }
    if collected {
        let total = slices.iter().map(|s| s.len()).sum();
        crate::bridge::string::utf8_str_from_slices(ruby, slices.iter().copied(), total)
    } else {
        let total = texts().map(<[u8]>::len).sum();
        crate::bridge::string::utf8_str_from_slices(ruby, texts(), total)
    }
}

/// The 1-based source line for `node`, or `None` when unknown.
pub fn node_line(rb_doc: Value, node: RawNode) -> Result<Option<usize>, Error> {
    with_html_parsed_known(rb_doc, |p| p.node_line(node))
}

/// A method receiver already checked to be an HTML node or HTML Document.
///
/// The field is the receiver's opaque key, not a raw pointer: a long-lived
/// wrapper stores one, and a Document receiver's key is minted for the call.
/// Its view resolves it against the receiver's document without a check
/// ([`HtmlView::node`]) - the receiver's own Document, kept alive by the
/// argument, is what makes that sound - while [`with_arg_node`] is the
/// checked boundary for a node argument.
pub struct HtmlSelf {
    pub value: Value,
    key: HtmlNodeKey,
    /// The keepalive Document (the receiver itself for a Document), as the
    /// handle its content is read through - and written, by `HtmlEdit`.
    pub(super) document: DocHandle,
}

impl crate::bridge::rb::TryConvert for HtmlSelf {
    fn try_convert(ruby: &Ruby, value: Value) -> Result<Self, Error> {
        let (key, document) = html_node_key(ruby, value)?;
        Ok(HtmlSelf {
            value,
            key,
            document: doc_known(document),
        })
    }
}

impl HtmlSelf {
    /// The keepalive Document (the receiver itself for a Document).
    #[inline]
    pub fn document(&self) -> Value {
        self.document.value()
    }

    /// The receiver's document, borrowed for reading: the guard the
    /// receiver's node and document are read through ([`HtmlView`]).
    ///
    /// The receiver is a method argument, which Ruby keeps reachable for the
    /// call and which keeps its document alive; the view borrows `self`, so
    /// it ends with the call. It is a counted borrow, which an edit of the
    /// same document refuses while it is held. A Ruby result may be built
    /// with it held: every allocation that can raise returns `Err`, so the
    /// guard drops on the way out ([`HtmlView::text`],
    /// [`node_set_from_read`](crate::bridge::node_set::node_set_from_read)).
    #[inline]
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raw_node_crossing,
            reason = "the receiver's key, as the node its own read resolves"
        )
    )]
    pub fn read(&self) -> Result<HtmlView<'_>, Error> {
        Ok(HtmlView {
            read: html_read(&self.document)?,
            raw: self.key.raw_node(),
            document: self.document(),
        })
    }

    /// The receiver's node as the boundary handle, for the mutators.
    #[inline]
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raw_node_crossing,
            reason = "the receiver's node, for a mutator to resolve inside its edit"
        )
    )]
    pub fn raw(&self) -> RawNode {
        self.key.raw_node()
    }
}

/// A receiver's document borrowed for reading, with the receiver's node:
/// [`HtmlSelf::read`].
pub struct HtmlView<'a> {
    read: HtmlRead<'a>,
    raw: RawNode,
    document: Value,
}

impl HtmlView<'_> {
    /// The node `f` finds from the receiver's, wrapped. The read ends before
    /// the wrap, which needs nothing of it: the found node leaves as a
    /// [`RawNode`], and the wrap mints its key in a read of its own.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raw_node_crossing,
            reason = "the node a read finds, wrapped once the read is over"
        )
    )]
    pub fn wrap(
        self,
        ruby: &Ruby,
        f: impl FnOnce(HtmlNode<'_>) -> Option<HtmlNode<'_>>,
    ) -> Result<Option<Value>, Error> {
        let found = f(self.node()).map(|n| (RawNode::from(n), n.node_type()));
        let document = self.document;
        drop(self);
        found
            .map(|(n, t)| wrap_html_node_of_type(ruby, n, t, document))
            .transpose()
    }

    /// The nodes `f` lists from the receiver's (the element ones alone when
    /// `elements_only`), as a NodeSet built with the read held
    /// ([`node_set_from_read`](crate::bridge::node_set::node_set_from_read)).
    /// `f` hands each node to the sink it is given.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raw_node_crossing,
            reason = "the nodes a read lists, as the words a NodeSet holds, pushed with the read held"
        )
    )]
    pub fn set(
        self,
        ruby: &Ruby,
        elements_only: bool,
        f: impl for<'n> Fn(HtmlNode<'n>, &mut dyn FnMut(HtmlNode<'n>)),
    ) -> Result<Value, Error> {
        let document = self.document;
        crate::bridge::node_set::node_set_from_read(ruby, self, document, |view, out| {
            f(view.node(), &mut |n| {
                if !elements_only || n.element().is_some() {
                    out(RawNode::from(n).into());
                }
            });
        })
    }

    /// `#content`: the receiver's text, in one read - the root element's for
    /// a Document (the DOM's null is not what callers want), its descendants'
    /// for an element or fragment, its own data for the other kinds.
    ///
    /// An element's comes from the per-document text index when it is built
    /// (`HtmlParsed::ensure_text_index`): the contiguous, document-order run of
    /// its descendants' text slices, so no per-extraction tree walk - the walk
    /// is otherwise the dominant, cache-bound cost. A node the index does not
    /// serve (a fragment's, or before the index is due, or after a build OOM)
    /// is walked ([`subtree_text`]). The String is made with the read held.
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raw_node_crossing,
            reason = "the node whose text is read, resolved again in the same read"
        )
    )]
    pub fn text(self, ruby: &Ruby) -> Result<Value, Error> {
        let target = {
            let n = self.node();
            match n.node_type() {
                NodeType::Document => n.document_root(),
                _ => Some(n),
            }
            .map(RawNode::from)
        };
        let Some(raw) = target else {
            drop(self);
            return utf8_str(ruby, b"").map(|s| s.as_value());
        };
        let node = self.read.node(raw);
        if !matches!(
            node.node_type(),
            NodeType::Element | NodeType::DocumentFragment
        ) {
            /* Character data and attributes lend their own bytes; the other
             * kinds (a doctype) have none. */
            let text = crate::bridge::string::str_from_read(
                ruby,
                self,
                crate::bridge::string::StrKind::Utf8,
                |v| v.read.node(raw).own_text(),
            )?;
            return match text {
                Some(text) => Ok(text),
                None => utf8_str(ruby, b"").map(|s| s.as_value()),
            };
        }
        let p = self.read.parsed();
        /* A build that failed closed, or one not due yet: walk. */
        let run = p.ensure_text_index().ok().and_then(|()| p.text_slices(raw));
        let Some(run) = run else {
            return subtree_text(ruby, &self.read, node);
        };
        /* The slices are this document's text, valid UTF-8 by the text-input
         * contract, borrowed from its index for the copy. */
        crate::bridge::string::utf8_str_from_slices(ruby, run.iter(), run.total())
    }

    /// The receiver's node. Unchecked: the receiver's key was minted from a
    /// node of the document it keeps alive.
    #[inline]
    pub fn node(&self) -> HtmlNode<'_> {
        self.read.node(self.raw)
    }

    /// The receiver's document.
    #[inline]
    pub fn doc(&self) -> HtmlDoc<'_> {
        self.read.doc()
    }

    /// The parse handle, with its indexes.
    #[inline]
    pub fn parsed(&self) -> &crate::html_dom::post_parse::HtmlParsed {
        self.read.parsed()
    }
}

/// Run `f` over the HTML node (or Document node) `v` wraps, resolved against
/// `v`'s own Document - the checked boundary for a node ARGUMENT, as
/// [`HtmlSelf::read`] is for a receiver. The handle lives only as long as
/// `f`, inside the document's borrow ([`with_html_node`]), so `f` must not
/// run Ruby code, and a node it finds leaves as a [`RawNode`], to be wrapped
/// after.
///
/// `Err(TypeError)` for anything that is not an HTML node or HTML Document.
pub fn with_arg_node<R>(
    ruby: &Ruby,
    v: Value,
    f: impl FnOnce(HtmlNode<'_>) -> R,
) -> Result<R, Error> {
    let (key, document) = html_node_key(ruby, v)?;
    with_html_node(document, key, f)
}

/* ------------------------------------------------------------------ *
 * indexing a child list                                               *
 * ------------------------------------------------------------------ */

impl ChildListHost for HtmlSelf {
    type Tree<'a> = HtmlTree<'a>;
    fn document(&self) -> Value {
        self.document()
    }
    fn with_children<R>(
        &self,
        list: ChildList,
        f: impl for<'a> FnOnce(Children<'a, HtmlTree<'a>>) -> R,
    ) -> Result<R, Error> {
        let view = self.read()?;
        let tree = HtmlTree::of(view.doc());
        Ok(f(Children {
            tree: &tree,
            parent: view.node(),
            list,
        }))
    }
    fn wrap_child(&self, ruby: &Ruby, token: Option<usize>) -> Result<Option<Value>, Error> {
        token
            .and_then(RawNode::from_word)
            .map(|n| wrap_html_node(ruby, n, self.document()))
            .transpose()
    }
}
