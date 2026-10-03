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
use crate::init::{
    CLASS_DOCUMENT, CLASS_HTML_ATTR, CLASS_HTML_CDATA_SECTION, CLASS_HTML_COMMENT,
    CLASS_HTML_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT, CLASS_HTML_NODE,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_TEXT, CLASS_XML_DOCUMENT,
};
use crate::lexbor::adapter::html::{
    ForeignNode, HtmlNode, HtmlNodeKey, HtmlNodeMut, Insertion, NodeType, Place, PreInsertError,
    RawDoc, RawNode,
};
use crate::lexbor::fragment::import_with_fixup;

use crate::bridge::string::{RubyData, RubyText};
use crate::bridge::wrapper::*;
use crate::lexbor::adapter::html::{HtmlDoc, HtmlElementMut};
use crate::lexbor::adapter::AdapterOom;

/* ---- the document's own bytes and text ---- */

/// A UTF-8 Ruby String copied from bytes the parsed document lends.
///
/// Safe by the text-input contract: parsing sanitizes invalid UTF-8 to U+FFFD,
/// so everything a document's readers hand over is valid UTF-8. That contract
/// belongs to this module - the one that ran the parser - which is why the
/// unsafe of `ruby_str_from_utf8` is discharged here.
pub fn dom_str(bytes: &[u8]) -> Value {
    // SAFETY: valid UTF-8 by the text-input contract; the String copies it.
    unsafe { value(crate::bridge::string::ruby_str_from_utf8(bytes)) }
}

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
    with_html_parsed_known(rb_doc, |p| {
        // SAFETY: `node` is a live node of this document.
        unsafe { p.node_line(node) }
    })
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
    /* SAFETY: a `RawNode` is live - the safe constructors are `From<HtmlNode>`
     * and `From<Building*>`, and the only raw one, `from_ptr`, is unsafe. */
    let handle = unsafe { node.as_node() };
    let node_type = handle.node_type();
    if node_type == NodeType::Document {
        return Ok(document);
    }
    let klass = HTML_NODE_CLASSES.class_for(node_type);
    crate::bridge::wrapper::wrap_cached(&HTML_NODE_TYPE, klass, node, document)
}

impl crate::bridge::wrapper::NodeHandleSource for RawNode {
    fn identity(&self) -> usize {
        self.as_ptr() as usize
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
    with_html_parsed_known(document, |parsed| {
        // SAFETY: `node` is a live node of `document`, vouched by the caller.
        unsafe { parsed.mint_key(node) }
    })
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
        let raw = RawNode::from(html_doc_unwrap(rb_node)?);
        let key = with_html_parsed_known(rb_node, |parsed| {
            // SAFETY: `raw` is the document node of `rb_node`'s live document.
            unsafe { parsed.mint_key(raw) }
        })
        .map_err(|ForeignNode| foreign_node_error())?;
        return Ok((key, rb_node));
    }
    let nd: &NodeData = HTML_NODE_TYPE.get(&rb_node)?;
    let key = nd.node.html().ok_or_else(uninitialized)?;
    // SAFETY: `nd.document` is the live Document the wrapper marks.
    let document = unsafe { value(nd.document) };
    Ok((key, document))
}

/// A method receiver already checked to be an HTML node or HTML Document.
///
/// The field is the receiver's opaque key, not a raw pointer: a long-lived
/// wrapper stores one, and a Document receiver's key is minted for the call.
/// `node()` resolves it without a check - the receiver's own Document, kept
/// alive by the argument, is what makes that sound - while `with_node` is the
/// checked boundary.
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
        // SAFETY: the receiver keeps `key`'s owner document alive for this
        // call, and `key` was minted from a live node of it.
        unsafe { self.key.raw_node().as_node() }
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

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// The receiver cleared for an edit - not frozen, its document not under
/// evaluation - and the PROOF of it: [`edit`] is the only way to build one and
/// [`HtmlEdit::node`] the only way to spend it. The XML side's `Editing`.
///
/// The checks and the index drop are two steps on purpose. The checks come
/// first, so a frozen receiver is reported before a bad argument. The drop
/// comes last, once every argument is converted: converting one runs its
/// `#to_s`, which is arbitrary Ruby, and a query there rebuilt the indexes from
/// the tree the edit was about to change - after which `#text` read the text
/// the edit had released, and `//p` found the nodes it had removed.
pub struct HtmlEdit<'a> {
    this: &'a HtmlSelf,
}

/// Clear the receiver for an edit. Convert every argument after this and
/// before [`HtmlEdit::node`].
pub fn edit(this: &HtmlSelf) -> Result<HtmlEdit<'_>, Error> {
    crate::bridge::ruby::check_frozen(this.value)?;
    ensure_document_mutable(this.document)?;
    /* Before any argument is converted: see `account_growth`. */
    crate::bridge::wrapper::account_growth(this.document);
    Ok(HtmlEdit { this })
}

impl<'a> HtmlEdit<'a> {
    /// The receiver's node, read-only, for a check that no argument can
    /// change (its node type).
    pub fn node_type(&self) -> NodeType {
        self.this.node().node_type()
    }

    /// The mutable handle, with the document's indexes dropped. From here to
    /// the change nothing may run Ruby - every argument is already converted -
    /// so nothing can rebuild them.
    ///
    /// Both guards are checked again, since an argument's `#to_s` ran between
    /// `edit` and here: one that froze the receiver had its edit go through.
    /// `edit` checked first only so a frozen receiver is still reported ahead
    /// of a bad argument. It takes the token, so a caller cannot reach for the
    /// handle a second time after running Ruby. What it cannot stop is Ruby
    /// run while the handle is held - the handle's lifetime is the receiver's,
    /// not the token's - so a caller keeps the span from here to the change
    /// to engine calls and checks that call no Ruby (`insert` reads its
    /// argument's node and frozen flag there, and nothing more).
    ///
    /// It counts as a change to a child list ([`record_edit`]); an
    /// attribute edit takes [`HtmlEdit::node_for_attributes`] instead, and a
    /// character-data edit [`HtmlEdit::node_for_data`].
    pub fn node(self) -> Result<HtmlNodeMut<'a>, Error> {
        self.mutable(EditKind::ChildList)
    }

    /// [`HtmlEdit::node`] for an edit of ATTRIBUTES only - an element's, or an
    /// Attr node's value - which changes no child list: it counts towards the
    /// attribute version ([`record_edit`]) instead of the tree
    /// version.
    pub fn node_for_attributes(self) -> Result<HtmlNodeMut<'a>, Error> {
        self.mutable(EditKind::Attributes)
    }

    /// [`HtmlEdit::node`] for an edit of a Text, Comment, CDATA or PI node's
    /// DATA, which changes no child list and no attribute: no version counts
    /// it (see [`EditKind::CharacterData`]).
    pub fn node_for_data(self) -> Result<HtmlNodeMut<'a>, Error> {
        self.mutable(EditKind::CharacterData)
    }

    fn mutable(self, kind: EditKind) -> Result<HtmlNodeMut<'a>, Error> {
        crate::bridge::ruby::check_frozen(self.this.value)?;
        ensure_document_mutable(self.this.document)?;
        invalidate_indexes(self.this.document);
        record_edit(self.this.document, kind);
        // SAFETY: the receiver is not frozen and no XPath evaluation is
        // reading its document - both checked just now.
        Ok(unsafe { HtmlNodeMut::assume_mutable(self.this.raw().as_node()) })
    }
}

/// A detached copy of `src` in `doc` - `<template>` contents included - or a
/// `Makiri::Error` naming `what` failed ("import node", "clone node"). The one
/// copy every import, adopt and clone makes, so none can grow its own variant.
///
/// # Safety
/// `doc` must be a live document, held by the caller for the call.
pub unsafe fn import_copy(
    doc: RawDoc,
    src: HtmlNode<'_>,
    deep: bool,
    what: &str,
) -> Result<RawNode, Error> {
    import_with_fixup(doc, RawNode::from(src), deep)
        .map_err(|_| makiri_error(format!("failed to {what}")))
}

/// Copy `node` into `doc`, for a node that came from another document.
fn adopt_copy<'d>(doc: RawDoc, node: HtmlNode<'_>) -> Result<HtmlNode<'d>, Error> {
    // SAFETY: `doc` is a live document, the caller's target.
    let imp = unsafe { import_copy(doc, node, true, "import node") }?;
    // SAFETY: a node just imported into `doc`, which outlives this call.
    Ok(unsafe { imp.as_node() })
}

/// Take the node `src` wraps out of the document it came from, so the whole
/// thing reads as the move the DOM says appendChild performs - and drop that
/// document's indexes, which still list it. A structural change to a document
/// invalidates ITS indexes; this is one, made from another document's method.
fn adopt_release(src: Value) -> Result<(), Error> {
    with_arg_node(src, |node| {
        /* SAFETY: the source document was cleared for editing by
         * `take_incoming` before anything was copied out of it. */
        release_from_tree(unsafe { HtmlNodeMut::assume_mutable(node) });
    })?;
    /* After the borrow `with_arg_node` held: dropping them borrows again. */
    let src_doc = keepalive_document(src)?;
    invalidate_indexes(src_doc);
    record_edit(src_doc, EditKind::ChildList);
    Ok(())
}

fn release_from_tree(node: HtmlNodeMut<'_>) {
    if node.node().node_type() == NodeType::DocumentFragment {
        /* A fragment contributes its children; the DOM leaves a spliced one
         * empty, so empty the source rather than detaching it. */
        while let Some(c) = node.first_child() {
            c.detach();
        }
    } else if node.parent().is_some() {
        node.detach();
    }
}

/// `node.add_child(other)` and its siblings: put `rb_incoming` at `place`
/// relative to the receiver - moved within the document, or adopted from its
/// own - and hand back what is now in the tree: the argument, or for an adopted
/// node its copy.
///
/// Every rule is checked before anything changes (see
/// [`Insertion::check`], the shared `crate::dom_rules`); after that only the adoption copy can fail, and it
/// too runs before a link is touched.
pub fn insert(this: &HtmlSelf, rb_incoming: Value, place: Place) -> Result<Value, Error> {
    let target = edit(this)?.node()?;
    let (key, incoming_doc) = html_node_key(rb_incoming)?;
    /* The argument is relinked too - `place` changes its parent and siblings, and
     * an adoption removes it from its own document - so a frozen argument is a
     * frozen node being modified. The receiver check alone let it through, which
     * made `a.remove` raise and `span.add_child(a)` not, for the same effect on
     * `a`. Reaches the nodes the caller NAMED; a fragment's children cannot be
     * checked, because frozenness lives on the Ruby object and there is no map
     * from a node back to its wrapper. */
    crate::bridge::ruby::check_frozen(rb_incoming)?;
    /* The argument, resolved against its own Document, for the checks, the
     * copy or move, and the placing - none of which runs Ruby or wraps a
     * node. Releasing an adopted original and wrapping its copy borrow a
     * document again, so they come after. */
    let (placed, adopted) = with_html_node(incoming_doc, key, |incoming| {
        Insertion::new(target.node(), place, incoming)
            .and_then(|i| i.check())
            .map_err(|e| refused(e, place))?;
        let (node, adopted) = take_incoming(target, incoming_doc, incoming)?;
        target.place(node, place);
        Ok::<_, Error>((RawNode::from(node.node()), adopted))
    })??;
    if !adopted {
        return Ok(rb_incoming);
    }
    adopt_release(rb_incoming)?;
    wrap_html_node(placed, this.document)
}

/// A refused insertion, worded. The one place these messages live.
fn refused(e: PreInsertError, place: Place) -> Error {
    use crate::dom_rules::{Hierarchy as H, Violation};
    makiri_error(match e {
        PreInsertError::NoParent if place == Place::Replace => {
            "cannot replace a node with no parent"
        }
        PreInsertError::NoParent => "cannot add a sibling to a node with no parent",
        /* Unreachable through `Insertion::new`, which takes the reference child
         * from the parent it names; worded all the same. */
        PreInsertError::Rule(Violation::NotFound) => {
            "the reference node is not a child of the parent"
        }
        PreInsertError::Rule(Violation::HierarchyRequest(h)) => match h {
            H::ParentNotContainer => {
                "only a document, a document fragment or an element can have children"
            }
            H::Ancestor => "cannot insert a node into its own subtree",
            H::AttributeNode => "an attribute node cannot be inserted into the tree",
            H::DocumentNode => "a document node cannot be inserted into the tree",
            H::UnsupportedNode => "this kind of node cannot be inserted into the tree",
            H::DoctypeParent => "a doctype node can only be a child of the document",
            H::DuplicateDoctype => "the document already has a doctype",
            H::DoctypeAfterElement | H::ElementBeforeDoctype => {
                "a doctype must precede the document element"
            }
            H::SecondDocumentElement => "the document already has a root element",
            H::TextUnderDocument => "text cannot be a child of the document",
        },
    })
}

/// The node to put in the tree for `incoming`: itself, taken out of where it
/// was, or - from another document - a copy made in `target`'s, with `true`
/// for "release the original once the copy is in" (see [`adopt_release`]).
/// `incoming_doc` is the argument's own Document.
fn take_incoming<'d>(
    target: HtmlNodeMut<'d>,
    incoming_doc: Value,
    incoming: HtmlNode<'_>,
) -> Result<(HtmlNodeMut<'d>, bool), Error> {
    if !target.node().same_document(incoming) {
        /* Adopting takes the node out of the document it came from, so that
         * document changes too - refuse before anything is copied. */
        ensure_document_mutable(incoming_doc)?;
        let copy = adopt_copy(RawDoc::from(target.node().owner_document()), incoming)?;
        // SAFETY: a copy this call just made in `target`'s document.
        return Ok((unsafe { HtmlNodeMut::assume_mutable(copy) }, true));
    }
    // SAFETY: a node of `target`'s document, which `edit` cleared.
    let incoming = unsafe { HtmlNodeMut::assume_mutable(RawNode::from(incoming).as_node()) };
    if incoming.parent().is_some() {
        incoming.detach();
    }
    Ok((incoming, false))
}

/* ------------------------------------------------------------------ *
 * verified strings into Lexbor                                        *
 * ------------------------------------------------------------------ *
 * The node methods live in `glue::html_node::mutate`. A checked view reads as
 * a plain `&str` (`RubyStr`'s `Deref`: it holds its String locked), and each
 * primitive passes it straight to a Lexbor call that copies what it keeps. A
 * primitive answers what Lexbor answered; the method words the error. */

/// The Lexbor document behind an HTML Document receiver, for a factory - so,
/// like every other change to a document, refused while an XPath evaluation
/// with a handler is reading it.
pub fn owning_doc(rb_self: &Value) -> Result<HtmlDoc<'_>, Error> {
    let doc = html_doc_unwrap(*rb_self)?;
    ensure_document_mutable(*rb_self)?;
    // SAFETY: a live HTML Document, kept alive by `rb_self` for this call.
    Ok(unsafe { doc.as_doc() })
}

/// `el[name] = value`; `Err` when Lexbor could not store it.
pub fn set_attribute(
    el: HtmlElementMut<'_>,
    name: &RubyText,
    value: &RubyData,
) -> Result<(), AdapterOom> {
    el.set_attribute(name.as_bytes(), value.as_bytes())
        .map(drop)
}

/// Set the attribute `qname` in namespace `ns` (`None` = none), matching an
/// existing one on (namespace, local name) - the DOM key - rather than on the
/// qualified name. `Err` when Lexbor could not store it.
///
/// `ns` comes from `string::namespace_arg`, so an empty URI already reads as
/// `None`.
pub fn set_attribute_ns(
    el: HtmlElementMut<'_>,
    ns: Option<&[u8]>,
    qname: &RubyText,
    value: &RubyData,
) -> Result<(), AdapterOom> {
    el.set_attribute_ns(ns, qname.as_bytes(), value.as_bytes())
}

/// Remove the attribute `local` in namespace `ns` (`None` = none); whether
/// there was one. `ns` comes from `string::namespace_arg`.
pub fn remove_attribute_ns(el: HtmlElementMut<'_>, ns: Option<&[u8]>, local: &RubyText) -> bool {
    /* Looked up, not interned: a namespace the document never interned is
     * one no attribute here carries, so there is nothing to remove - and a
     * lookup can neither fail nor grow the table. */
    let want_ns = match ns {
        Some(uri) => {
            let doc = el.element().node().owner_document();
            let Some(id) = doc.lookup_ns(uri) else {
                return false;
            };
            Some(id)
        }
        None => None,
    };
    match el.element().attr_by_ns(want_ns, local.as_bytes()) {
        Some(attr) => {
            el.attr_remove(attr);
            true
        }
        None => false,
    }
}

/// `el.delete(name)`.
pub fn remove_attribute(el: HtmlElementMut<'_>, name: &RubyText) {
    el.remove_attribute(name.as_bytes());
}

/// `node.content = text`; `Err` when Lexbor could not store it.
pub fn set_text_content(node: HtmlNodeMut<'_>, text: &RubyData) -> Result<(), AdapterOom> {
    node.set_text_content(text.as_bytes())
}

/// A new element in `doc`.
pub fn create_element<'d>(doc: HtmlDoc<'d>, name: &RubyText) -> Option<RawNode> {
    doc.create_element(name.as_bytes()).map(RawNode::from)
}

/// A new element named `local` in namespace `ns` ("" = none), with `prefix`
/// ("" = none) - the DOM's createElementNS, once the caller has validated and
/// split the name.
pub fn create_element_ns<'d>(
    doc: HtmlDoc<'d>,
    local: &[u8],
    ns: &[u8],
    prefix: &[u8],
) -> Option<RawNode> {
    doc.create_element_ns(local, ns, prefix).map(RawNode::from)
}

/// A new Text node in `doc`.
pub fn create_text(doc: HtmlDoc<'_>, text: &RubyData) -> Option<RawNode> {
    doc.create_text(text.as_bytes()).map(RawNode::from)
}

/// A new Comment in `doc`.
pub fn create_comment(doc: HtmlDoc<'_>, text: &RubyData) -> Option<RawNode> {
    doc.create_comment(text.as_bytes()).map(RawNode::from)
}

/// A new ProcessingInstruction in `doc`.
pub fn create_pi(doc: HtmlDoc<'_>, target: &RubyText, data: &RubyText) -> Option<RawNode> {
    doc.create_pi(target.as_bytes(), data.as_bytes())
        .map(RawNode::from)
}

/// Whether `name` is one the DOM accepts for a doctype.
pub fn valid_doctype_name(name: &RubyText) -> bool {
    HtmlDoc::valid_doctype_name(name.as_bytes())
}

/// A new DocumentType in `doc`.
pub fn create_doctype(
    doc: HtmlDoc<'_>,
    name: &RubyText,
    public_id: Option<&RubyText>,
    system_id: Option<&RubyText>,
) -> Option<RawNode> {
    let (name, pub_id, sys_id) = (
        name.as_bytes(),
        public_id.map(|v| v.as_bytes()),
        system_id.map(|v| v.as_bytes()),
    );
    doc.create_doctype(name, pub_id, sys_id).map(RawNode::from)
}
