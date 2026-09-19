//! The Ruby <-> Lexbor seam for HTML nodes: wrapping a Lexbor node for Ruby
//! and back, the checked receiver (`HtmlSelf`), and the tree and attribute
//! edits the node methods make.
//!
//! A value read out of a Ruby wrapper becomes a live Lexbor handle here, and
//! the conversion is unsafe; the lexbor side stays Ruby-free.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, Value};

use crate::bridge::ruby::{nil, value};
use crate::init::{
    CLASS_DOCUMENT, CLASS_HTML_ATTR, CLASS_HTML_CDATA_SECTION, CLASS_HTML_COMMENT,
    CLASS_HTML_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT_TYPE, CLASS_HTML_ELEMENT, CLASS_HTML_NODE,
    CLASS_HTML_PROCESSING_INSTRUCTION, CLASS_HTML_TEXT, CLASS_XML_DOCUMENT, EXC_ERROR,
};
use crate::lexbor::adapter::html::{
    check_document_child_order, DocumentChildOrderError, HtmlNode, HtmlNodeMut, RawDoc, RawNode,
    TYPE_ATTRIBUTE, TYPE_CDATA, TYPE_COMMENT, TYPE_DOCTYPE, TYPE_DOCUMENT, TYPE_ELEMENT,
    TYPE_FRAGMENT, TYPE_PI, TYPE_TEXT,
};
use crate::lexbor::fragment::import_with_fixup;

use crate::bridge::fragment::fragment_error;
use crate::bridge::string::{HtmlSource, RubyData, RubyText};
use crate::bridge::wrapper::*;
use crate::lexbor::adapter::html::{HtmlDoc, HtmlElementMut, ScratchElement, NS_UNDEF};
use crate::lexbor::fragment::{Emit, TransientFragment};

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
    with_parsed_known(document, |p| {
        if let Some((slices, total)) = p.text_slices(node.as_lxb()) {
            // SAFETY: the slices point into this document's arena (the index
            // borrows them from `p`) and are copied into the String before the
            // borrow ends; nothing here runs Ruby.
            let built = unsafe { crate::bridge::string::ruby_str_from_slices(slices, total) };
            found = Some(built.map(|v| {
                // SAFETY: the String `ruby_str_from_slices` just built, live and
                // on this frame.
                unsafe { value(v) }
            }));
        }
    });
    found.transpose()
}

/// The element that owns `attr` through the attr->owner index.
///
/// `Err` when the index cannot be built (out of memory) - distinct from a node
/// the index does not know, which is `Ok(None)`. The owner is borrowed for as
/// long as the caller borrows `rb_doc`, the Document that keeps it alive.
pub fn attribute_owner(rb_doc: &Value, attr: RawNode) -> Result<Option<HtmlNode<'_>>, Error> {
    with_parsed(*rb_doc, |p| match p.dom_index() {
        None => Err(Error::new(
            EXC_ERROR.exception(),
            "could not build the attribute index (out of memory)",
        )),
        // SAFETY: an owner the live index answers is a live node of this document.
        Some(i) => Ok(i.owner_of(attr).map(|o| unsafe { o.as_node() })),
    })?
}

/// The 1-based source line for `node`, or 0 when unknown.
pub fn node_line(rb_doc: Value, node: RawNode) -> usize {
    with_parsed_known(rb_doc, |p| {
        // SAFETY: `node` is a live node of this document.
        unsafe { p.node_line(node.as_ptr() as *const _) }
    })
}

/* ------------------------------------------------------------------ *
 * the HTML node front door                                           *
 * ------------------------------------------------------------------ */

/// Wrap a live HTML node handle into its `Makiri::HTML::*` leaf.
///
/// A DOCUMENT node maps back onto the Ruby Document rather than getting a
/// second wrapper; a node type with no specific leaf (entity/notation, which
/// Lexbor's HTML parser does not produce) falls back to `Makiri::HTML::Node`.
pub fn wrap_html_node(node: RawNode, document: Value) -> Value {
    /* SAFETY: a `RawNode` is live - the safe constructors are `From<HtmlNode>`
     * and `From<Building*>`, and the only raw one, `from_ptr`, is unsafe. */
    let handle = unsafe { node.as_node() };
    let node_type = handle.node_type();
    if node_type == TYPE_DOCUMENT {
        return document;
    }

    let klass = match node_type {
        TYPE_ELEMENT => CLASS_HTML_ELEMENT.raw(),
        TYPE_ATTRIBUTE => CLASS_HTML_ATTR.raw(),
        TYPE_TEXT => CLASS_HTML_TEXT.raw(),
        TYPE_COMMENT => CLASS_HTML_COMMENT.raw(),
        TYPE_CDATA => CLASS_HTML_CDATA_SECTION.raw(),
        TYPE_PI => CLASS_HTML_PROCESSING_INSTRUCTION.raw(),
        TYPE_DOCTYPE => CLASS_HTML_DOCUMENT_TYPE.raw(),
        TYPE_FRAGMENT => CLASS_HTML_DOCUMENT_FRAGMENT.raw(),
        _ => CLASS_HTML_NODE.raw(),
    };

    /* The Document is stored after the wrap: see `TypedType::wrap`. */
    // SAFETY: a fresh wrapper; the store closure only moves a live VALUE in.
    unsafe {
        value(HTML_NODE_TYPE.wrap(
            klass,
            |nd| nd.node = node.as_ptr(),
            |nd| nd.document = document.as_raw(),
        ))
    }
}

/// The HTML node handle behind an HTML node or HTML Document.
///
/// `Err(TypeError)` for an XML node or Document: the typed-data check is against
/// [`HTML_NODE_TYPE`], which an XML node (wrapped under `XML_NODE_TYPE`) does
/// not satisfy.
pub fn html_node_unwrap(rb_node: Value) -> Result<RawNode, Error> {
    if rb_node.is_kind_of(CLASS_DOCUMENT.class()) {
        if rb_node.is_kind_of(CLASS_XML_DOCUMENT.class()) {
            return Err(Error::new(
                magnus::Ruby::get()
                    .expect("under the GVL")
                    .exception_type_error(),
                "expected an HTML node, got a Makiri::XML::Document",
            ));
        }
        return Ok(html_doc_unwrap(rb_node)?.into());
    }
    let nd: &NodeData = HTML_NODE_TYPE.get(&rb_node)?;
    RawNode::from_ptr(nd.node).ok_or_else(uninitialized)
}

fn uninitialized() -> Error {
    Error::new(
        magnus::Ruby::get()
            .expect("under the GVL")
            .exception_type_error(),
        "uninitialized HTML node",
    )
}

/// A method receiver already checked to be an HTML node or HTML Document.
#[derive(Clone, Copy)]
pub struct HtmlSelf {
    pub value: Value,
    raw: RawNode,
    /// The keepalive Document (the receiver itself for a Document).
    pub document: Value,
}

impl magnus::TryConvert for HtmlSelf {
    fn try_convert(value: Value) -> Result<Self, Error> {
        let raw = html_node_unwrap(value)?;
        let document = keepalive_document(value)?;
        Ok(HtmlSelf {
            value,
            raw,
            document,
        })
    }
}

impl HtmlSelf {
    /// The receiver's node handle, for the length of this method call.
    ///
    /// The receiver is a method argument, which Ruby keeps reachable for the
    /// call and which keeps its document alive; the borrow of `self` ends the
    /// handle with the call.
    #[inline]
    pub fn node(&self) -> HtmlNode<'_> {
        // SAFETY: the receiver keeps the node's document alive for this call.
        unsafe { self.raw.as_node() }
    }

    /// The receiver's node as the boundary handle, for the mutators.
    #[inline]
    pub fn raw(&self) -> RawNode {
        self.raw
    }
}

/// An HTML node argument, for the length of the borrow of `v`.
///
/// `Err(TypeError)` for anything that is not an HTML node or HTML Document.
pub fn arg_node(v: &Value) -> Result<HtmlNode<'_>, Error> {
    // SAFETY: `v` is a method argument, which keeps its node's document alive.
    Ok(unsafe { html_node_unwrap(*v)?.as_node() })
}

/// [`wrap_html_node`] for an optional handle: nil for None.
pub fn wrap_node(node: Option<HtmlNode<'_>>, document: Value) -> Value {
    match node {
        Some(n) => wrap_html_node(RawNode::from(n), document),
        None => nil(),
    }
}

/* ------------------------------------------------------------------ *
 * structural mutation                                                *
 * ------------------------------------------------------------------ */

/// A mutable handle to the receiver, after the frozen and evaluation guards.
///
/// A node the caller has frozen is immutable (FrozenError), and a document an
/// XPath handler is being evaluated over refuses to change.
pub fn edit(this: &HtmlSelf) -> Result<HtmlNodeMut<'_>, Error> {
    crate::bridge::ruby::check_frozen(this.value)?;
    ensure_document_mutable(this.document)?;
    // SAFETY: the checks above are exactly what the type asks for - the
    // receiver is not frozen, and no XPath evaluation is reading its document.
    Ok(unsafe { HtmlNodeMut::assume_mutable(this.raw().as_node()) })
}

/// Where an insert puts its node, which is what lets [`splice_or_insert`] hold
/// the fragment rule in one place.
#[derive(Clone, Copy)]
pub enum Insert {
    Child,
    Before,
    After,
}

impl Insert {
    #[inline]
    fn put(self, anchor: HtmlNodeMut<'_>, node: HtmlNodeMut<'_>) {
        match self {
            Insert::Child => anchor.insert_child(node),
            Insert::Before => anchor.insert_before(node),
            Insert::After => anchor.insert_after(node),
        }
    }
}

fn err(msg: &str) -> Error {
    Error::new(EXC_ERROR.exception(), msg.to_owned())
}

/// Copy `node` into `doc`, for a node that came from another document.
fn adopt_copy<'d>(doc: RawDoc, node: HtmlNode<'_>) -> Result<HtmlNode<'d>, Error> {
    // SAFETY: `doc` is a live document and `node` its caller's live source.
    let imp = unsafe { import_with_fixup(doc, RawNode::from(node), true) }
        .ok_or_else(|| err("failed to import node"))?;
    // SAFETY: a node just imported into `doc`, which outlives this call.
    Ok(unsafe { imp.as_node() })
}

/// Take the node `src` wraps out of the document it came from, so the whole
/// thing reads as the move the DOM says appendChild performs - and drop that
/// document's indexes, which still list it. A structural change to a document
/// invalidates ITS indexes; this is one, made from another document's method.
fn adopt_release(src: Value) -> Result<(), Error> {
    /* SAFETY: the source document was cleared for editing by `prepare_insert`
     * before anything was copied out of it. */
    let node = unsafe { HtmlNodeMut::assume_mutable(arg_node(&src)?) };
    release_from_tree(node);
    invalidate_indexes(keepalive_document(src)?);
    Ok(())
}

fn release_from_tree(node: HtmlNodeMut<'_>) {
    if node.node().node_type() == TYPE_FRAGMENT {
        /* A fragment contributes its children; the DOM leaves a spliced one
         * empty, so empty the source rather than detaching it. */
        while let Some(c) = node.first_child() {
            c.detach();
        }
    } else if node.parent().is_some() {
        node.detach();
    }
}

/// Validate that `rb_incoming` may be placed relative to `reference`, detach it
/// from any current parent, and return the node to actually insert.
pub fn prepare_insert<'d>(
    reference: HtmlNodeMut<'d>,
    rb_incoming: Value,
) -> Result<(HtmlNodeMut<'d>, Option<Value>), Error> {
    // SAFETY: `unwrap` checked `rb_incoming` is an HTML node, and the caller
    // holds it, which keeps its document alive for the call.
    let incoming = unsafe { html_node_unwrap(rb_incoming)?.as_node() };

    if incoming.node_type() == TYPE_ATTRIBUTE {
        return Err(err("an attribute node cannot be inserted into the tree"));
    }
    /* `incoming` must not be an inclusive ancestor of `reference`. */
    let mut p = Some(reference.node());
    while let Some(n) = p {
        if n == incoming {
            return Err(err("cannot insert a node into its own subtree"));
        }
        p = n.parent();
    }
    let doc = reference.node().owner_document_handle();
    if doc.as_ptr() != incoming.owner_document_handle().as_ptr() {
        /* Adopting takes the node out of the document it came from, so that
         * document changes too - refuse before anything is copied. */
        ensure_document_mutable(keepalive_document(rb_incoming)?)?;
        let copy = adopt_copy(doc, incoming)?;
        // SAFETY: a copy this call just made in `reference`'s document.
        return Ok((
            unsafe { HtmlNodeMut::assume_mutable(copy) },
            Some(rb_incoming),
        ));
    }
    // SAFETY: same document as `reference`, which the caller cleared.
    let incoming = unsafe { HtmlNodeMut::assume_mutable(incoming) };
    if incoming.parent().is_some() {
        incoming.detach();
    }
    Ok((incoming, None))
}

/// Finish an insertion: for an adopted node, take it out of its old document
/// (see [`adopt_release`]); then the value the verb hands back - its argument,
/// or for an adopted node the copy now in the tree.
pub fn finish_insert(
    rb_self: Value,
    rb_arg: Value,
    inserted: HtmlNodeMut<'_>,
    adopt_from: Option<Value>,
) -> Result<Value, Error> {
    match adopt_from {
        None => Ok(rb_arg),
        Some(src) => {
            adopt_release(src)?;
            Ok(wrap_html_node(
                RawNode::from(inserted.node()),
                keepalive_document(rb_self)?,
            ))
        }
    }
}

/// Validate WHATWG doctype ordering before links are changed.
pub fn guard_doc_child_order(
    parent: Option<HtmlNode<'_>>,
    before: Option<HtmlNode<'_>>,
    exclude: Option<HtmlNode<'_>>,
    incoming: HtmlNode<'_>,
) -> Result<(), Error> {
    check_document_child_order(parent, before, exclude, incoming).map_err(|e| match e {
        DocumentChildOrderError::DoctypeParent => {
            err("a doctype node can only be a child of the document")
        }
        DocumentChildOrderError::DuplicateDoctype => err("the document already has a doctype"),
        DocumentChildOrderError::DoctypeAfterElement
        | DocumentChildOrderError::ElementBeforeDoctype => {
            err("a doctype must precede the document element")
        }
    })
}

/// Insert `node` relative to `anchor`, or - when `node` is a document fragment -
/// splice its children there in order.
pub fn splice_or_insert<'d>(
    mut anchor: HtmlNodeMut<'d>,
    node: HtmlNodeMut<'d>,
    insert: Insert,
    advance: bool,
) {
    if node.node().node_type() != TYPE_FRAGMENT {
        insert.put(anchor, node);
        return;
    }
    while let Some(c) = node.first_child() {
        c.detach();
        insert.put(anchor, c);
        if advance {
            anchor = c; /* keep document order after the reference node */
        }
    }
}

/* ------------------------------------------------------------------ *
 * verified strings into Lexbor                                        *
 * ------------------------------------------------------------------ *
 * The node methods live in `glue::html_node::mutate`, which is unsafe-free. The
 * one unsafe they would need is reading a verified String's bytes
 * (`RubyStr::bytes`), sound only while no Ruby code runs - so the reads are
 * here, each passed straight to a Lexbor call that copies what it keeps and
 * runs no Ruby. A primitive answers what Lexbor answered; the method words the
 * error. */

/// The Lexbor document behind an HTML Document receiver, for a factory - so,
/// like every other change to a document, refused while an XPath evaluation
/// with a handler is reading it.
pub fn owning_doc(rb_self: &Value) -> Result<HtmlDoc<'_>, Error> {
    let doc = html_doc_unwrap(*rb_self)?;
    ensure_document_mutable(*rb_self)?;
    // SAFETY: a live HTML Document, kept alive by `rb_self` for this call.
    Ok(unsafe { doc.as_doc() })
}

/// `el[name] = value`; false when Lexbor could not store it.
pub fn set_attribute(el: HtmlElementMut<'_>, name: &RubyText, value: &RubyData) -> bool {
    // SAFETY: see the section comment.
    unsafe { el.set_attribute(name.bytes(), value.bytes()) }.is_some()
}

/// Set the attribute `qname` in namespace `ns` (nil or "" = none), matching an
/// existing one on (namespace, local name) - the DOM key - rather than on the
/// qualified name. False when Lexbor could not store it.
pub fn set_attribute_ns(
    el: HtmlElementMut<'_>,
    ns: Option<&RubyText>,
    qname: &RubyText,
    value: &RubyData,
) -> bool {
    // SAFETY: see the section comment.
    let (qname, value) = unsafe { (qname.bytes(), value.bytes()) };
    /* An empty URI is no namespace: it names the attribute the unprefixed way. */
    // SAFETY: as above.
    let ns = ns.filter(|v| v.len() != 0).map(|v| unsafe { v.bytes() });
    let want_ns = intern_ns(el, ns.unwrap_or(&[]));
    let local = match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    };
    match el.element().find_attr_ns(want_ns, local) {
        Some(existing) => existing.set_value(value),
        None => el.append_attribute(ns, qname, value),
    }
}

/// Remove the attribute `local` in namespace `ns` (nil or "" = none); whether
/// there was one.
pub fn remove_attribute_ns(
    el: HtmlElementMut<'_>,
    ns: Option<&RubyText>,
    local: &RubyText,
) -> bool {
    // SAFETY: see the section comment.
    let want_ns = match ns.filter(|v| v.len() != 0) {
        Some(nv) => intern_ns(el, unsafe { nv.bytes() }),
        None => NS_UNDEF,
    };
    // SAFETY: as above.
    match el.element().find_attr_ns(want_ns, unsafe { local.bytes() }) {
        Some(attr) => {
            el.attr_remove(attr);
            true
        }
        None => false,
    }
}

/// `uri` interned in `el`'s document, for a (namespace, local name) lookup.
fn intern_ns(el: HtmlElementMut<'_>, uri: &[u8]) -> usize {
    let doc = el.element().node().owner_document();
    // SAFETY: the element's own Document, live for this call.
    unsafe { HtmlDoc::from_raw(doc) }.map_or(NS_UNDEF, |d| d.intern_ns(uri))
}

/// `el.delete(name)`.
pub fn remove_attribute(el: HtmlElementMut<'_>, name: &RubyText) {
    // SAFETY: see the section comment.
    el.remove_attribute(unsafe { name.bytes() });
}

/// Rename `el` in place, keeping its identity; false when Lexbor could not
/// intern the name.
pub fn rename(el: HtmlElementMut<'_>, name: &RubyText) -> bool {
    // SAFETY: the element's own Document, and the section comment.
    let scratch =
        unsafe { ScratchElement::create(el.element().node().owner_document(), name.bytes()) };
    match scratch {
        Some(scratch) => {
            scratch.rename(el);
            true
        }
        None => false,
    }
}

/// `node.content = text`; false when Lexbor could not store it.
pub fn set_text_content(node: HtmlNodeMut<'_>, text: &RubyData) -> bool {
    // SAFETY: see the section comment.
    node.set_text_content(unsafe { text.bytes() })
}

/// A new element in `doc`.
pub fn create_element<'d>(doc: HtmlDoc<'d>, name: &RubyText) -> Option<RawNode> {
    // SAFETY: see the section comment.
    doc.create_element(unsafe { name.bytes() })
        .map(RawNode::from)
}

/// A new Text node in `doc`.
pub fn create_text(doc: HtmlDoc<'_>, text: &RubyData) -> Option<RawNode> {
    // SAFETY: see the section comment.
    doc.create_text(unsafe { text.bytes() }).map(RawNode::from)
}

/// A new Comment in `doc`.
pub fn create_comment(doc: HtmlDoc<'_>, text: &RubyData) -> Option<RawNode> {
    // SAFETY: see the section comment.
    doc.create_comment(unsafe { text.bytes() })
        .map(RawNode::from)
}

/// A new ProcessingInstruction in `doc`.
pub fn create_pi(doc: HtmlDoc<'_>, target: &RubyText, data: &RubyText) -> Option<RawNode> {
    // SAFETY: see the section comment.
    doc.create_pi(unsafe { target.bytes() }, unsafe { data.bytes() })
        .map(RawNode::from)
}

/// Whether `name` is one the DOM accepts for a doctype.
pub fn valid_doctype_name(name: &RubyText) -> bool {
    // SAFETY: see the section comment.
    HtmlDoc::valid_doctype_name(unsafe { name.bytes() })
}

/// A new DocumentType in `doc`.
pub fn create_doctype(
    doc: HtmlDoc<'_>,
    name: &RubyText,
    public_id: Option<&RubyText>,
    system_id: Option<&RubyText>,
) -> Option<RawNode> {
    // SAFETY: see the section comment.
    let (name, pub_id, sys_id) = unsafe {
        (
            name.bytes(),
            public_id.map(|v| v.bytes()),
            system_id.map(|v| v.bytes()),
        )
    };
    doc.create_doctype(name, pub_id, sys_id).map(RawNode::from)
}

/* ---- fragments for inner_html= / outer_html= ---- */

/// Parse `rb_html` as a fragment in the context of `context`. Nothing is
/// changed yet: a String that fails to convert or parse leaves the tree as it
/// was, and the caller splices the result in with [`splice_fragment`].
pub fn parse_fragment_for(
    context: HtmlNode<'_>,
    rb_html: Value,
) -> Result<TransientFragment, Error> {
    /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
    let html = crate::bridge::ruby::string_of(rb_html)?.as_value();
    let src = HtmlSource::from_ruby(html)?;
    // SAFETY: `context` is a live element, and the bytes are read by the parse
    // alone, which runs no Ruby.
    unsafe { TransientFragment::parse(src.bytes(), src.known_valid(), RawNode::from(context)) }
        .map_err(fragment_error)
}

/// Where [`splice_fragment`] puts the fragment's children.
#[derive(Clone, Copy)]
pub enum Place {
    /// As the last children of the node.
    Append,
    /// Just before the node, under its parent.
    Before,
}

/// Import `frag`'s children into `at`'s document, placed by `place`.
pub fn splice_fragment(
    frag: TransientFragment,
    at: HtmlNodeMut<'_>,
    place: Place,
) -> Result<(), Error> {
    let node = RawNode::from(at.node());
    let emit = match place {
        Place::Append => Emit::Append(node),
        Place::Before => Emit::Before(node),
    };
    // SAFETY: `at` is a live node the caller cleared for editing, and its
    // document is the one the children go into.
    if !unsafe { frag.import_into(at.node().owner_document_handle(), &emit) } {
        return Err(err("failed to import a fragment child"));
    }
    Ok(())
}
