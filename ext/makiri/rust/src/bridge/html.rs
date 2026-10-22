//! The Ruby <-> Lexbor seam for HTML nodes: wrapping a Lexbor node for Ruby
//! and back, the checked receiver (`HtmlSelf`), and the tree and attribute
//! edits the node methods make.
//!
//! A value read out of a Ruby wrapper becomes a live Lexbor handle here, and
//! the conversion is unsafe; the lexbor side stays Ruby-free.

#![allow(unsafe_code)]

use crate::bridge::ruby::makiri_error;
use magnus::{Error, Value};

use crate::bridge::ruby::value;
use crate::html_dom::html::{
    ForeignNode, HtmlNode, HtmlNodeKey, HtmlTree, Insertion, NodeType, Place, RawNode,
};
use crate::init::{
    CLASS_DOCUMENT, CLASS_HTML_ATTR, CLASS_HTML_CDATA_SECTION, CLASS_HTML_COMMENT,
    CLASS_HTML_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT, CLASS_HTML_NODE,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_TEXT, CLASS_XML_DOCUMENT,
};

use crate::bridge::dom::DocPtr;
use crate::bridge::string::{RubyData, RubyText};
use crate::bridge::wrapper::*;
use crate::child_index::{ChildList, Children, TokenTree};
use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNewNodes};
use crate::html_dom::AdapterOom;

/* ---- the document's own bytes and text ---- */

/// The indexed descendant text of `node` as one Ruby String.
///
/// `Ok(None)` when the text index cannot serve this node (it is outside the
/// indexed tree, e.g. a fragment, or its build failed closed) - the caller then
/// walks. `Err` only when building the String fails.
pub fn text_index_string(document: Value, node: RawNode) -> Result<Option<Value>, Error> {
    let mut found: Option<Result<Value, Error>> = None;
    with_html_parsed_known(document, |p| {
        if p.ensure_text_index().is_err() {
            return; /* build failed closed: walk instead */
        }
        if let Some(run) = p.text_slices(node) {
            // SAFETY: valid UTF-8 by the text-input contract (the slices are
            // this document's text, borrowed from its index for the copy).
            let built =
                unsafe { crate::bridge::string::ruby_str_from_slices(run.iter(), run.total()) };
            found = Some(built.map(|v| {
                // SAFETY: the String `ruby_str_from_slices` just built, live and
                // on this frame.
                unsafe { value(v) }
            }));
        }
    });
    found.transpose()
}

/// The 1-based source line for `node`, or `None` when unknown.
pub fn node_line(rb_doc: Value, node: RawNode) -> Option<usize> {
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
/// Lexbor's HTML parser does not produce) falls back to `Makiri::HTML::Node`.
pub fn wrap_html_node(node: RawNode, document: Value) -> Result<Value, Error> {
    // The caller hands `node` with the Document it belongs to.
    let handle = crate::bridge::wrapper::html_doc(&document).node(node);
    let node_type = handle.node_type();
    if node_type == NodeType::Document {
        return Ok(document);
    }
    let klass = HTML_NODE_CLASSES.class_for(node_type);
    crate::bridge::wrapper::wrap_cached(&HTML_NODE_TYPE, klass, node, document)
}

impl crate::bridge::wrapper::NodeHandleSource for RawNode {
    fn identity(&self) -> usize {
        self.word()
    }

    fn into_handle(self, document: Value) -> NodeHandle {
        NodeHandle::Html(mint_html_key(self, document))
    }
}

/// The key for `node` under `document`, minted only when a wrapper is actually
/// built. Its caller vouches `node` is a node of `document`, so a refusal is a
/// broken invariant, not a bad input.
#[allow(
    clippy::expect_used,
    reason = "wrap_html_node's callers vouch the node belongs to the document"
)]
fn mint_html_key(node: RawNode, document: Value) -> HtmlNodeKey {
    with_html_parsed_known(document, |parsed| parsed.mint_key(node))
        .expect("wrap_html_node: the node must belong to the document it is wrapped under")
}

fn uninitialized() -> Error {
    crate::bridge::ruby::type_error("uninitialized HTML node")
}

/// The key and keepalive Document of an HTML node or HTML Document.
///
/// `Err(TypeError)` for an XML node or Document: the typed-data check is
/// against [`HTML_NODE_TYPE`], which an XML node (wrapped under
/// `XML_NODE_TYPE`) does not satisfy. The Document node is keyed too: Lexbor sets its `owner_document` to itself, so
/// `mint_key` accepts it.
pub(in crate::bridge) fn html_node_key(rb_node: Value) -> Result<(HtmlNodeKey, Value), Error> {
    if crate::bridge::ruby::is_kind_of(rb_node, &CLASS_DOCUMENT) {
        if crate::bridge::ruby::is_kind_of(rb_node, &CLASS_XML_DOCUMENT) {
            return Err(crate::bridge::ruby::type_error(
                "expected an HTML node, got a Makiri::XML::Document",
            ));
        }
        let doc = html_doc_unwrap(rb_node)?;
        // SAFETY: the live document of `rb_node`, which the caller holds.
        let raw = unsafe { crate::bridge::dom::document_node(doc) };
        let key = with_html_parsed_known(rb_node, |parsed| parsed.mint_key(raw))
            .map_err(|ForeignNode| foreign_node_error())?;
        return Ok((key, rb_node));
    }
    let nd: &NodeData = HTML_NODE_TYPE.get(&rb_node)?;
    let key = nd.node.html().ok_or_else(uninitialized)?;
    // SAFETY: the field of the wrapper `rb_node`, whose mark marks it; the
    // argument keeps the wrapper reachable.
    let document = unsafe { nd.document.get() };
    Ok((key, document))
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

impl magnus::TryConvert for HtmlSelf {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let (key, document) = html_node_key(value)?;
        Ok(HtmlSelf {
            value,
            key,
            document,
        })
    }
}

impl HtmlSelf {
    /// The receiver's node, for the length of this method call.
    ///
    /// The receiver is a method argument, which Ruby keeps reachable for the
    /// call and which keeps its document - and so the node `key` names -
    /// alive; the borrow of `self` ends the handle with the call.
    #[inline]
    pub fn node(&self) -> HtmlNode<'_> {
        let doc = self.doc();
        // The receiver keeps `key`'s owner document alive for this call, and
        // `key` was minted from a live node of it.
        doc.node(self.key.raw_node())
    }

    /// The receiver's document, for the length of this method call - made
    /// afresh on each call and never stored, so no handle of it is held
    /// across an edit of the same document.
    #[inline]
    pub fn doc(&self) -> HtmlDoc<'_> {
        crate::bridge::wrapper::html_doc(&self.document)
    }

    /// The receiver's node as the boundary handle, for the mutators.
    #[inline]
    pub fn raw(&self) -> RawNode {
        self.key.raw_node()
    }

    /// Run `f` over the receiver's node, resolving the stored key against the
    /// receiver's document - the CHECKED boundary, where a key paired with the
    /// wrong Document is refused rather than dereferenced. `node()` is the
    /// unchecked fast path for a receiver whose key and Document are already
    /// known to agree (they always do: the wrapper was minted that way).
    pub fn with_node<R>(
        &self,
        f: impl FnOnce(HtmlNode<'_>) -> Result<R, Error>,
    ) -> Result<R, Error> {
        with_html_node(self.document, self.key, f)?
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
pub fn with_arg_node<R>(v: Value, f: impl FnOnce(HtmlNode<'_>) -> R) -> Result<R, Error> {
    let (key, document) = html_node_key(v)?;
    with_html_node(document, key, f)
}

/// [`wrap_html_node`] for an optional handle.
pub fn wrap_node(node: Option<HtmlNode<'_>>, document: Value) -> Result<Option<Value>, Error> {
    node.map(|n| wrap_html_node(RawNode::from(n), document))
        .transpose()
}

/* ------------------------------------------------------------------ *
 * indexing a child list                                               *
 * ------------------------------------------------------------------ */

/// The arena's tree for [`crate::child_index`]: a node's word is its token.
/// The links are [`HtmlTree`]'s, the raw ones `#children` follows, so the two
/// agree - a `<template>`'s own (empty) child list included.
impl<'d> TokenTree for HtmlTree<'d> {
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
    ) -> R {
        let tree = HtmlTree::of(self.doc());
        f(Children {
            tree: &tree,
            parent: self.node(),
            list,
        })
    }
    fn wrap_child<'a>(&self, n: Option<HtmlNode<'a>>) -> Result<Option<Value>, Error>
    where
        Self: 'a,
    {
        wrap_node(n, self.document)
    }
}

/* ------------------------------------------------------------------ *
 * resolving a long-lived key                                          *
 * ------------------------------------------------------------------ */

/// A key paired with the wrong Document: an internal invariant broken, not a
/// bad argument, so `InternalError` rather than `Makiri::Error`.
fn foreign_node_error() -> Error {
    Error::new(
        crate::init::EXC_INTERNAL_ERROR.exception(),
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
    })
}

/// The node `key` names, as a reference into `rb_doc`'s document, after the
/// same check [`with_html_node`] makes.
///
/// What an edit takes for a node of the document it is changing: the edit
/// holds that document by `&mut` (`HtmlDocMut`), so the node is read through
/// that borrow - never through a second handle - and a reference is all that
/// crosses. The one place a checked key leaves the bridge's handles as a
/// reference.
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
/// comes last, once every argument is converted: converting one runs its
/// `#to_s`, which is arbitrary Ruby, and a query there rebuilt the indexes from
/// the tree the edit was about to change - after which `#text` read the text
/// the edit had released, and `//p` found the nodes it had removed.
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
pub fn edit(this: &mut HtmlSelf) -> Result<HtmlEdit<'_>, Error> {
    crate::bridge::ruby::check_frozen(this.value)?;
    check_attr_owner_frozen(this)?;
    ensure_document_mutable(this.document)?;
    /* Before any argument is converted: see `account_growth`. */
    crate::bridge::wrapper::account_growth(this.document);
    Ok(HtmlEdit { this })
}

/// An Attr receiver's edit changes its OWNER's attribute list, so a frozen
/// owner refuses it as it refuses `delete` - checked with the receiver's own
/// frozen flag, before and after the arguments are converted.
fn check_attr_owner_frozen(this: &HtmlSelf) -> Result<(), Error> {
    let node = this.node();
    match (node.node_type(), node.parent()) {
        (NodeType::Attribute, Some(owner)) => {
            crate::bridge::wrapper::check_node_frozen(this.document, RawNode::from(owner))
        }
        _ => Ok(()),
    }
}

impl<'a> HtmlEdit<'a> {
    /// The receiver's node, read-only, for a check that no argument can
    /// change (its node type).
    pub fn node_type(&self) -> NodeType {
        self.this.node().node_type()
    }

    /// Lend the document for the change, with the receiver's node, and drop
    /// its indexes first. From here to the end of `f` nothing may run Ruby -
    /// every argument is already converted - so nothing can rebuild them, and
    /// nothing can read the document while `f` holds it.
    ///
    /// Both guards are checked again, since an argument's `#to_s` ran between
    /// `edit` and here: one that froze the receiver had its edit go through.
    /// `edit` checked first only so a frozen receiver is still reported ahead
    /// of a bad argument. It takes the token, so a caller cannot reach for the
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
        crate::bridge::ruby::check_frozen(self.this.value)?;
        check_attr_owner_frozen(self.this)?;
        ensure_document_mutable(self.this.document)?;
        record_edit(self.this.document, kind);
        // SAFETY: the receiver is not frozen and no XPath evaluation is
        // reading its document - both checked just now - and `f` runs no Ruby
        // (this method's contract with its callers), so the document is this
        // edit's alone while `f` holds it. `edit` drops the indexes.
        let mut doc = unsafe { crate::bridge::wrapper::html_doc_known(self.this.document).edit() };
        Ok(f(&mut doc, self.this.raw()))
    }
}

/// Refuse a factory on `rb_doc` - not an HTML Document, or one an XPath
/// evaluation with a handler is reading - before any argument is converted, so
/// that refusal wins over a bad argument. [`new_node`] checks again.
pub fn check_owning_doc(rb_doc: &Value) -> Result<(), Error> {
    html_doc_unwrap(*rb_doc)?;
    ensure_document_mutable(*rb_doc)
}

/// Lend `rb_doc`'s document to `f` to make a DETACHED node - the factories'
/// and the copies' gate. A new node is in no tree, so no index changes and no
/// receiver can be frozen; what is refused is a document an XPath evaluation
/// with a handler is reading, whose arena the walk borrows. `f` must run no
/// Ruby, as [`HtmlEdit::with_tree`]'s must not.
pub fn new_node<R>(rb_doc: &Value, f: impl FnOnce(&mut HtmlNewNodes<'_>) -> R) -> Result<R, Error> {
    lend_document(rb_doc, |doc| {
        // SAFETY: `lend_document`'s: a live HTML Document, kept alive by
        // `rb_doc` for this call, not under evaluation, and `f` runs no Ruby.
        // The loan ends with this closure, before the document is read again.
        let mut d = unsafe { doc.edit_new_nodes() };
        f(&mut d)
    })
}

/// Lend `rb_doc`'s document to `f` for a change to its tree, attributes or
/// data: the parse handle's `edit`, which drops the indexes and runs a due
/// compaction.
fn edit_document<R>(rb_doc: &Value, f: impl FnOnce(&mut HtmlDocMut<'_>) -> R) -> Result<R, Error> {
    lend_document(rb_doc, |doc| {
        // SAFETY: as `new_node`'s.
        let mut d = unsafe { doc.edit() };
        f(&mut d)
    })
}

/// Check `rb_doc` can be lent - an HTML Document no evaluation is reading -
/// hand its document to `lend`, and report what the loan grew it by.
fn lend_document<R>(rb_doc: &Value, lend: impl FnOnce(DocPtr) -> R) -> Result<R, Error> {
    let doc = html_doc_unwrap(*rb_doc)?;
    ensure_document_mutable(*rb_doc)?;
    let r = lend(doc);
    /* A factory grows the document without an edit after it - a namespace
     * of a megabyte, a fragment imported - so it reports here, after the
     * loan: the arguments were converted to copies before, and what it made
     * is still a bare node, so a collection here can free nothing it needs. */
    crate::bridge::wrapper::account_growth(*rb_doc);
    Ok(r)
}

/// Lend the document of `rb_doc` - not the receiver's - for the part of an
/// edit that changes it: an adoption's release of the original. The change
/// is recorded first; the lend's `edit` drops its indexes.
fn edit_other_document<R>(
    rb_doc: Value,
    f: impl FnOnce(&mut HtmlDocMut<'_>) -> R,
) -> Result<R, Error> {
    ensure_document_mutable(rb_doc)?;
    record_edit(rb_doc, EditKind::ChildList);
    edit_document(&rb_doc, f)
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
pub fn insert(this: &mut HtmlSelf, rb_incoming: Value, place: Place) -> Result<Value, Error> {
    let document = this.document;
    let edit = edit(this)?;
    let (key, incoming_doc) = html_node_key(rb_incoming)?;
    /* The argument is relinked too - `place` changes its parent and siblings, and
     * an adoption removes it from its own document - so a frozen argument is a
     * frozen node being modified. The receiver check alone let it through, which
     * made `a.remove` raise and `span.add_child(a)` not, for the same effect on
     * `a`. Reaches the nodes the caller NAMED; a fragment's children cannot be
     * checked, because frozenness lives on the Ruby object and there is no map
     * from a node back to its wrapper. */
    crate::bridge::ruby::check_frozen(rb_incoming)?;
    /* An adoption changes the argument's own document too. Refused here, so
     * every refusal about the argument comes before the edit is recorded, as
     * the XML side's does. */
    ensure_document_mutable(incoming_doc)?;
    /* The argument resolved against its own Document - the checked boundary -
     * and kept as a reference: in the receiver's document it is read through
     * the edit's own borrow, never through a second one. */
    let incoming = node_ref(incoming_doc, key)?;
    let same = crate::bridge::ruby::same_value(incoming_doc, document);
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
            with_html_node(incoming_doc, key, |src| {
                Insertion::new(d.node(target), place, src)
                    .and_then(|i| i.check())
                    .map_err(crate::bridge::dom_error::pre_insert_error)?;
                d.import(src, true)
                    .map_err(|_| makiri_error("failed to import node"))
            })??
        };
        d.place(target, node, place);
        Ok((node, !same))
    })??;
    if !adopted {
        return Ok(rb_incoming);
    }
    /* Take the original out of the document it came from, so the whole thing
     * reads as the move the DOM says appendChild performs - and drop that
     * document's indexes, which still list it. */
    edit_other_document(incoming_doc, |d| release_from_tree(d, incoming))?;
    wrap_html_node(placed, document)
}

/* ------------------------------------------------------------------ *
 * verified strings into Lexbor                                        *
 * ------------------------------------------------------------------ *
 * The node methods live in `glue::html_node::mutate`. A checked view reads as
 * a plain `&str` (`RubyStr`'s `Deref`, over its own copy of the bytes), and each
 * primitive passes it straight to a Lexbor call that copies what it keeps. A
 * primitive answers what Lexbor answered; the method words the error. */

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
