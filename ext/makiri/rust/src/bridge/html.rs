//! The Ruby <-> arena seam for HTML nodes: wrapping an arena node for Ruby
//! and back, the checked receiver (`HtmlSelf`), and the tree and attribute
//! edits the node methods make.
//!
//! A value read out of a Ruby wrapper becomes a live arena handle here, and
//! the conversion is unsafe; the arena side (`html::dom`) stays Ruby-free.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{Error, ReprValue, Value};
use crate::bridge::ruby::makiri_error;

use crate::html_dom::html::{
    ForeignNode, HtmlNode, HtmlNodeKey, HtmlTree, Insertion, NodeType, Place, RawNode,
};
use crate::init::{
    CLASS_DOCUMENT, CLASS_HTML_ATTR, CLASS_HTML_CDATA_SECTION, CLASS_HTML_COMMENT,
    CLASS_HTML_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT, CLASS_HTML_NODE,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_TEXT, CLASS_XML_DOCUMENT,
};

use crate::bridge::dom::HtmlRead;
use crate::bridge::string::{utf8_str, RubyData, RubyText};
use crate::bridge::wrapper::*;
use crate::child_index::{ChildList, Children, TokenTree};
use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNewNodes};
use crate::html_dom::AdapterOom;

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
    use crate::falloc::VecGrow;
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

/* ------------------------------------------------------------------ *
 * the HTML node front door                                           *
 * ------------------------------------------------------------------ */

/// The `Makiri::HTML::*` leaves, by node type.
static HTML_NODE_CLASSES: NodeClasses = NodeClasses {
    node: &CLASS_HTML_NODE,
    element: &CLASS_HTML_ELEMENT,
    attr: &CLASS_HTML_ATTR,
    text: &CLASS_HTML_TEXT,
    comment: &CLASS_HTML_COMMENT,
    cdata: &CLASS_HTML_CDATA_SECTION,
    pi: &CLASS_HTML_PROCESSING_INSTRUCTION,
    doctype: &CLASS_HTML_DOCUMENT_TYPE,
    fragment: &CLASS_HTML_DOCUMENT_FRAGMENT,
};

/// Wrap a live HTML node handle into its `Makiri::HTML::*` leaf.
///
/// A DOCUMENT node maps back onto the Ruby Document rather than getting a
/// second wrapper; a node type with no specific leaf (entity/notation, which
/// the HTML parser does not produce) falls back to `Makiri::HTML::Node`.
pub fn wrap_html_node(ruby: &Ruby, node: RawNode, document: Value) -> Result<Value, Error> {
    // The caller hands `node` with the Document it belongs to.
    let node_type = html_read(&document)?.node(node).node_type();
    wrap_html_node_of_type(ruby, node, node_type, document)
}

/// [`wrap_html_node`] for a node that must be one of `document`'s: an
/// `InternalError` when it is not (another document's, or a stale word).
pub(in crate::bridge) fn wrap_html_node_checked(
    ruby: &Ruby,
    node: RawNode,
    document: Value,
) -> Result<Value, Error> {
    /* A wrapper in the table was made for this word, checked then - and a
     * word names one node of one document for the document's life - so a
     * hit needs no check, no type and no class. */
    if let Some(cached) = crate::bridge::wrapper::cached_node(document, &node)? {
        return Ok(cached);
    }
    let node_type = html_read(&document)?
        .doc()
        .checked(node)
        .map(|n| n.node_type())
        .ok_or_else(|| crate::bridge::ruby::internal_error("a stored node of another document"))?;
    wrap_html_node_of_type(ruby, node, node_type, document)
}

/// [`wrap_html_node`] for a caller that read the node's type already, in the
/// read it found the node in - one read the fewer.
fn wrap_html_node_of_type(
    ruby: &Ruby,
    node: RawNode,
    node_type: NodeType,
    document: Value,
) -> Result<Value, Error> {
    if node_type == NodeType::Document {
        return Ok(document);
    }
    let klass = HTML_NODE_CLASSES.class_for(ruby, node_type)?;
    crate::bridge::wrapper::wrap_cached(ruby, &HTML_NODE_TYPE, klass, node, document)
}

impl crate::bridge::wrapper::NodeHandleSource for RawNode {
    fn index_and_stamp(&self) -> (u32, u32) {
        (self.index(), self.stamp())
    }

    fn into_handle(self, document: Value) -> NodeHandle {
        NodeHandle::Html(mint_html_key(self, document))
    }
}

/// The key for `node` under `document`, minted only when a wrapper is actually
/// built. Its caller vouches `node` is a node of `document`, so a refusal is a
/// broken invariant, not a bad input: a panic, which the trampoline raises as
/// `InternalError` (or, were an exit pending, resumes that first).
#[allow(
    clippy::expect_used,
    reason = "wrap_html_node's callers vouch the node belongs to the document"
)]
fn mint_html_key(node: RawNode, document: Value) -> HtmlNodeKey {
    /* `wrap_html_node` read the document a moment ago, with nothing run
     * between, so it can be read again. */
    with_html_parsed_known(document, |parsed| parsed.mint_key(node))
        .expect("wrap_html_node: the document must be readable again")
        .expect("wrap_html_node: the node must belong to the document it is wrapped under")
}

fn uninitialized() -> Error {
    crate::bridge::ruby::type_error("uninitialized HTML node")
}

/// The key and keepalive Document of an HTML node or HTML Document.
///
/// `Err(TypeError)` for an XML node or Document: the typed-data check is
/// against [`HTML_NODE_TYPE`], which an XML node (wrapped under
/// `XML_NODE_TYPE`) does not satisfy. The Document node is keyed too: it is its own `owner_document`, so
/// `mint_key` accepts it.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(raw_node_crossing, reason = "a Document's own node, minted as its key")
)]
pub(in crate::bridge) fn html_node_key(
    ruby: &Ruby,
    rb_node: Value,
) -> Result<(HtmlNodeKey, Value), Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_node, &CLASS_DOCUMENT) {
        if crate::bridge::ruby::is_kind_of(ruby, rb_node, &CLASS_XML_DOCUMENT) {
            return Err(crate::bridge::ruby::type_error(
                "expected an HTML node, got a Makiri::XML::Document",
            ));
        }
        html_doc_check(rb_node)?;
        let key = with_html_parsed_known(rb_node, |parsed| {
            parsed.mint_key(RawNode::from(parsed.doc().as_node()))
        })?
        .map_err(|ForeignNode| foreign_node_error())?;
        return Ok((key, rb_node));
    }
    let nd = crate::bridge::wrapper::read_node(&HTML_NODE_TYPE, rb_node)?;
    let key = nd.node.html().ok_or_else(uninitialized)?;
    Ok((key, nd.document))
}

/// A method receiver already checked to be an HTML node or HTML Document.
///
/// The field is the receiver's opaque key, not a raw pointer: a long-lived
/// wrapper stores one, and a Document receiver's key is minted for the call.
/// `node()` resolves it against the receiver's document without a check - the
/// receiver's own Document, kept alive by the argument, is what makes that
/// sound - while `with_node` is the checked boundary.
#[derive(Clone, Copy)]
pub struct HtmlSelf {
    pub value: Value,
    key: HtmlNodeKey,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl crate::bridge::rb::TryConvert for HtmlSelf {
    fn try_convert(ruby: &Ruby, value: Value) -> Result<Self, Error> {
        let (key, document) = html_node_key(ruby, value)?;
        Ok(HtmlSelf {
            value,
            key,
            document,
        })
    }
}

impl HtmlSelf {
    /// The receiver's document, borrowed for reading: the guard the
    /// receiver's node and document are read through ([`HtmlView`]).
    ///
    /// The receiver is a method argument, which Ruby keeps reachable for the
    /// call and which keeps its document alive; the view borrows `self`, so
    /// it ends with the call. It is a counted borrow, so it must not be held
    /// across a call that can raise (`rake unsafe:boundaries`): read what is
    /// needed, drop it, then build the Ruby result.
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
            document: self.document,
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
    /// The node `f` finds from the receiver's, wrapped - after the read is
    /// over, since wrapping allocates.
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
/// [`HtmlSelf::with_node`] is for a receiver. The handle lives only as long
/// as `f`, inside the document's borrow ([`with_html_node`]), so `f` must not
/// run Ruby or wrap a node of that document (wrapping mints a key, which
/// borrows the document again): hand a [`RawNode`] out and wrap after.
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

/// [`wrap_html_node`] for an optional handle.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "a node wrapped: the wrapper keeps its key, and its document alive"
    )
)]
pub fn wrap_node(
    ruby: &Ruby,
    node: Option<HtmlNode<'_>>,
    document: Value,
) -> Result<Option<Value>, Error> {
    node.map(|n| wrap_html_node(ruby, RawNode::from(n), document))
        .transpose()
}

/* ------------------------------------------------------------------ *
 * indexing a child list                                               *
 * ------------------------------------------------------------------ */

/// The arena's tree for [`crate::child_index`]: a node's word is its token.
/// The links are [`HtmlTree`]'s, the raw ones `#children` follows, so the two
/// agree - a `<template>`'s own (empty) child list included.
impl<'d> TokenTree for HtmlTree<'d> {
    #[cfg_attr(
        dylint_lib = "makiri_lints",
        allow(
            raw_node_crossing,
            reason = "the child-position memo's token, read back only through the checked lookup"
        )
    )]
    fn token(&self, n: HtmlNode<'d>) -> usize {
        RawNode::from(n).word()
    }
    fn node_of(&self, token: usize) -> Option<HtmlNode<'d>> {
        // `child_index` asks only for a token it recorded for a list of this
        // document (`ChildPositionMemo`'s invariant), and the lookup is the
        // checked one all the same: a stale or foreign word answers None,
        // which the memo reads as a miss, rather than another node.
        let raw = RawNode::from_word(token)?;
        self.doc()?.checked(raw)
    }
}

impl ChildListHost for HtmlSelf {
    type Tree<'a> = HtmlTree<'a>;
    fn document(&self) -> Value {
        self.document
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
            .map(|n| wrap_html_node(ruby, n, self.document))
            .transpose()
    }
}

/* ------------------------------------------------------------------ *
 * resolving a long-lived key                                          *
 * ------------------------------------------------------------------ */

/// A key paired with the wrong Document: an internal invariant broken, not a
/// bad argument, so `InternalError` rather than `Makiri::Error`.
fn foreign_node_error() -> Error {
    Error::new(
        &crate::init::EXC_INTERNAL_ERROR,
        "an HTML node key belongs to a different document",
    )
}

/// Run `f` over the node `key` names, after checking that node belongs to
/// `rb_doc`'s document.
///
/// `rb_doc` is the node's keepalive Document, and `key` was minted from a node
/// of that document, so the check fails only if a caller paired a key with the
/// wrong Document - hence `InternalError`. `f` runs inside the document borrow,
/// so it must not run Ruby that could re-enter this document, and must not
/// return a borrowed handle or slice.
pub(in crate::bridge) fn with_html_node<R>(
    rb_doc: Value,
    key: HtmlNodeKey,
    f: impl FnOnce(HtmlNode<'_>) -> R,
) -> Result<R, Error> {
    with_html_parsed_known(rb_doc, |parsed| {
        parsed
            .resolve(key)
            .map(f)
            .map_err(|ForeignNode| foreign_node_error())
    })?
}

/// The node `key` names, as a reference into `rb_doc`'s document, after the
/// same check [`with_html_node`] makes.
///
/// What an edit takes for a node of the document it is changing: the edit
/// holds that document by `&mut` (`HtmlDocMut`), so the node is read through
/// that borrow - never through a second handle - and a reference is all that
/// crosses. The one place a checked key leaves the bridge's handles as a
/// reference.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "a checked key as a reference into its document, for an edit that holds it"
    )
)]
pub(in crate::bridge) fn node_ref(rb_doc: Value, key: HtmlNodeKey) -> Result<RawNode, Error> {
    with_html_node(rb_doc, key, |n| RawNode::from(n))
}

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// The receiver cleared for an edit - not frozen, its document not under
/// evaluation - and the PROOF of it: [`edit`] is the only way to build one and
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
    crate::bridge::wrapper::account_growth(ruby, this.document);
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
        Some(owner) => crate::bridge::wrapper::check_node_frozen(this.document, owner),
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
        record_edit(self.this.document, kind);
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
    lend: impl FnOnce(&mut crate::bridge::dom::HtmlWrite<'_>) -> R,
) -> Result<R, Error> {
    html_doc_check(*rb_doc)?;
    /* The loan ends with this block, before the report below can run Ruby;
     * `lend` runs none. */
    let r = {
        let mut write = html_write(rb_doc)?;
        lend(&mut write)
    };
    /* A factory grows the document without an edit after it - a namespace
     * of a megabyte, a fragment imported - so it reports here, after the
     * loan: the arguments were converted to copies before, and what it made
     * is still a bare node, so a collection here can free nothing it needs. */
    crate::bridge::wrapper::account_growth(ruby, *rb_doc);
    Ok(r)
}

/// Take `node` out of its tree, the release half of an adoption: a fragment
/// contributes its children (the DOM leaves a spliced one empty), anything
/// else leaves its parent.
fn release_from_tree(d: &mut HtmlDocMut<'_>, node: RawNode) {
    let n = d.node(node);
    if n.node_type() == NodeType::DocumentFragment {
        d.detach_children(node);
    } else if n.parent().is_some() {
        d.detach(node);
    }
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
    let document = this.document;
    let edit = edit(ruby, this)?;
    let (key, incoming_doc) = html_node_key(ruby, rb_incoming)?;
    /* The argument is relinked too - `place` changes its parent and siblings, and
     * an adoption removes it from its own document - so a frozen argument is a
     * frozen node being modified. The receiver check alone let it through, which
     * made `a.remove` raise and `span.add_child(a)` not, for the same effect on
     * `a`. Reaches the nodes the caller NAMED; a fragment's children cannot be
     * checked, because frozenness lives on the Ruby object and there is no map
     * from a node back to its wrapper. */
    rb_incoming.check_frozen()?;
    /* The argument resolved against its own Document - the checked boundary -
     * and kept as a reference: in the receiver's document it is read through
     * the edit's own borrow, never through a second one. */
    let incoming = node_ref(incoming_doc, key)?;
    let same = crate::bridge::ruby::same_value(incoming_doc, document);
    /* From another document, the argument's own document is written too -
     * its original released once the copy is in - so it is taken for the
     * edit before anything changes: a refusal then comes before the copy is
     * placed rather than after, which would leave the copy in and the
     * original where it was. The source is read through the same borrow.
     * Nothing between here and the release runs Ruby. */
    let mut source = match same {
        true => None,
        false => Some(html_write(&incoming_doc)?),
    };
    let (placed, adopted) = edit.with_tree(|d, target| -> Result<(RawNode, bool), Error> {
        let node = if same {
            let insertion = Insertion::new(d.node(target), place, d.node(incoming))
                .map_err(crate::bridge::dom_error::pre_insert_error)?;
            insertion
                .check()
                .map_err(crate::bridge::dom_error::pre_insert_error)?;
            if insertion.unchanged() {
                return Ok((incoming, false));
            }
            if d.node(incoming).parent().is_some() {
                d.detach(incoming);
            }
            incoming
        } else {
            /* From another document: checked, then copied into this one - the
             * original is released below, once the copy is in. */
            let parsed = source
                .as_ref()
                .ok_or_else(|| {
                    crate::bridge::ruby::internal_error("an adoption without its source")
                })?
                .parsed();
            let src = parsed
                .resolve(key)
                .map_err(|ForeignNode| foreign_node_error())?;
            Insertion::new(d.node(target), place, src)
                .and_then(|i| i.check())
                .map_err(crate::bridge::dom_error::pre_insert_error)?;
            d.import(src, true)
                .map_err(|_| makiri_error("failed to import node"))?
        };
        d.place(target, node, place);
        Ok((node, !same))
    })??;
    let Some(mut source) = source.take().filter(|_| adopted) else {
        return Ok(rb_incoming);
    };
    /* Take the original out of the document it came from, so the whole thing
     * reads as the move the DOM says appendChild performs - and drop that
     * document's indexes, which still list it. Recorded before the change
     * (`record_edit`), through the borrow taken above, which cannot be
     * refused now. */
    record_edit(incoming_doc, EditKind::ChildList);
    release_from_tree(&mut source.edit(), incoming);
    drop(source);
    /* The document that grew is the receiver's, by the copy; the original's
     * only lost a link. */
    crate::bridge::wrapper::account_growth(ruby, document);
    wrap_html_node(ruby, placed, document)
}

/* ------------------------------------------------------------------ *
 * verified strings into the arena                                    *
 * ------------------------------------------------------------------ *
 * The node methods live in `glue::html_node::mutate`. A checked view reads as
 * a plain `&str` (`RubyStr`'s `Deref`, over its own copy of the bytes), and each
 * primitive passes it straight to an arena edit that copies what it keeps. A
 * primitive answers what the edit answered; the method words the error. */

/// `el[name] = value` on the element `el`; `Err` when it could not be stored.
pub fn set_attribute(
    d: &mut HtmlDocMut<'_>,
    el: RawNode,
    name: &RubyText,
    value: &RubyData,
) -> Result<(), AdapterOom> {
    d.set_attribute(el, name.as_bytes(), value.as_bytes())
}

/// Set the attribute `qname` in namespace `ns` (`None` = none), matching an
/// existing one on (namespace, local name) - the DOM key - rather than on the
/// qualified name. `Err` when it could not be stored.
///
/// `ns` comes from `string::namespace_arg`, so an empty URI already reads as
/// `None`.
pub fn set_attribute_ns(
    d: &mut HtmlDocMut<'_>,
    el: RawNode,
    ns: Option<&[u8]>,
    qname: &RubyText,
    value: &RubyData,
) -> Result<(), AdapterOom> {
    d.set_attribute_ns(el, ns, qname.as_bytes(), value.as_bytes())
}

/// Remove the attribute `local` in namespace `ns` (`None` = none) from the
/// element `el`; whether there was one. `ns` comes from
/// `string::namespace_arg`.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "the attribute found, removed in the same edit"
    )
)]
pub fn remove_attribute_ns(
    d: &mut HtmlDocMut<'_>,
    el: RawNode,
    ns: Option<&[u8]>,
    local: &RubyText,
) -> bool {
    /* Looked up, not interned: a namespace the document never interned is
     * one no attribute here carries, so there is nothing to remove - and a
     * lookup can neither fail nor grow the table. */
    let want_ns = match ns {
        Some(uri) => match d.read().lookup_ns(uri) {
            Some(id) => Some(id),
            None => return false,
        },
        None => None,
    };
    let found = d
        .node(el)
        .element()
        .and_then(|e| e.attr_by_ns(want_ns, local.as_bytes()))
        .map(|a| RawNode::from(a.node()));
    match found {
        Some(attr) => {
            d.attr_remove(el, attr);
            true
        }
        None => false,
    }
}

/// `el.delete(name)` on the element `el`.
pub fn remove_attribute(d: &mut HtmlDocMut<'_>, el: RawNode, name: &RubyText) {
    d.remove_attribute(el, name.as_bytes());
}

/// `node.content = text`; `Err` when it could not be stored.
pub fn set_text_content(
    d: &mut HtmlDocMut<'_>,
    node: RawNode,
    text: &RubyData,
) -> Result<(), AdapterOom> {
    d.set_text_content(node, text.as_bytes())
}

/// Whether `name` is one the DOM accepts for a doctype.
pub fn valid_doctype_name(name: &RubyText) -> bool {
    HtmlDoc::valid_doctype_name(name.as_bytes())
}
