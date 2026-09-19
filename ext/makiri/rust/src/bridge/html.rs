//! The Ruby <-> Lexbor seam for HTML nodes: wrapping a Lexbor node for Ruby
//! and back, the checked receiver (`HtmlSelf`), and the tree and attribute
//! edits the node methods make.
//!
//! A value read out of a Ruby wrapper becomes a live Lexbor handle here, and
//! the conversion is unsafe; the lexbor side stays Ruby-free.

#![allow(unsafe_code)]

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, Ruby, Value};

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

use crate::bridge::wrapper::*;

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
/// the index does not know, which is `Ok(None)`.
pub fn attribute_owner<'a>(rb_doc: Value, attr: RawNode) -> Result<Option<HtmlNode<'a>>, Error> {
    with_parsed(rb_doc, |p| match p.dom_index() {
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
    let nd: &NodeData = HTML_NODE_TYPE.get(rb_node)?;
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
pub fn edit<'a>(this: &HtmlSelf) -> Result<HtmlNodeMut<'a>, Error> {
    crate::bridge::ruby::check_frozen(this.value)?;
    ensure_document_mutable(this.document)?;
    // SAFETY: the checks above are exactly what the type asks for - the
    // receiver is not frozen, and no XPath evaluation is reading its document.
    Ok(unsafe { HtmlNodeMut::assume_mutable(this.raw().as_node()) })
}

/// Where an insert puts its node, which is what lets [`splice_or_insert`] hold
/// the fragment rule in one place.
#[derive(Clone, Copy)]
enum Insert {
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
fn adopt_copy(doc: RawDoc, node: HtmlNode<'_>) -> Result<HtmlNode<'static>, Error> {
    // SAFETY: `doc` is a live document and `node` its caller's live source.
    let imp = unsafe { import_with_fixup(doc, RawNode::from(node), true) }
        .ok_or_else(|| err("failed to import node"))?;
    // SAFETY: a node just imported into `doc`, which outlives this call.
    Ok(unsafe { imp.as_node() })
}

/// Take `node` out of the document it came from, so the whole thing reads as
/// the move the DOM says appendChild performs.
fn adopt_release(node: HtmlNodeMut<'_>) {
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
fn prepare_insert(
    reference: HtmlNodeMut<'_>,
    rb_incoming: Value,
) -> Result<(HtmlNodeMut<'static>, Option<Value>), Error> {
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

/// The value an insertion verb hands back: its argument, or - when the node was
/// adopted - the node now in the tree.
fn inserted_result(
    rb_self: Value,
    rb_arg: Value,
    inserted: HtmlNodeMut<'_>,
    adopt_from: Option<Value>,
) -> Result<Value, Error> {
    match adopt_from {
        None => Ok(rb_arg),
        Some(src) => {
            /* SAFETY: the source document was cleared for editing by
             * `prepare_insert` before anything was copied out of it. */
            adopt_release(unsafe { HtmlNodeMut::assume_mutable(arg_node(&src)?) });
            Ok(wrap_html_node(
                RawNode::from(inserted.node()),
                keepalive_document(rb_self)?,
            ))
        }
    }
}

/// Validate WHATWG doctype ordering before links are changed.
fn guard_doc_child_order(
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
fn splice_or_insert<'d>(
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

/// `node.add_child(child)` -> child.
pub fn add_child(_ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let parent = edit(&this)?;
    guard_doc_child_order(Some(parent.node()), None, None, arg_node(&rb_child)?)?;
    let (ins, adopt_from) = prepare_insert(parent, rb_child)?;
    splice_or_insert(parent, ins, Insert::Child, false);
    invalidate_indexes(this.document);
    inserted_result(rb_self, rb_child, ins, adopt_from)
}

/// `node << child` -> node (chainable).
pub fn lshift(ruby: &Ruby, this: HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    add_child(ruby, this, rb_child)?;
    Ok(rb_self)
}

/// `node.add_previous_sibling(node)` / `before` -> node.
pub fn before(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let reference = edit(&this)?;
    let Some(parent) = reference.parent() else {
        return Err(err("cannot add a sibling to a node with no parent"));
    };
    guard_doc_child_order(
        Some(parent.node()),
        Some(reference.node()),
        None,
        arg_node(&rb_node)?,
    )?;
    let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
    splice_or_insert(reference, ins, Insert::Before, false);
    invalidate_indexes(this.document);
    inserted_result(rb_self, rb_node, ins, adopt_from)
}

/// `node.add_next_sibling(node)` / `after` -> node.
pub fn after(_ruby: &Ruby, this: HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let reference = edit(&this)?;
    let Some(parent) = reference.parent() else {
        return Err(err("cannot add a sibling to a node with no parent"));
    };
    guard_doc_child_order(
        Some(parent.node()),
        reference.next().map(|n| n.node()),
        None,
        arg_node(&rb_node)?,
    )?;
    let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
    splice_or_insert(reference, ins, Insert::After, true);
    invalidate_indexes(this.document);
    inserted_result(rb_self, rb_node, ins, adopt_from)
}

/// `node.remove` / `node.unlink` -> node.
pub fn remove(_ruby: &Ruby, this: HtmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    let node = edit(&this)?;
    if node.node().node_type() == TYPE_ATTRIBUTE {
        return Err(err("use delete(name) to remove an attribute"));
    }
    if node.parent().is_some() {
        node.detach();
        invalidate_indexes(this.document);
    }
    Ok(rb_self)
}

/// `node.replace(other)` -> other.
pub fn replace(_ruby: &Ruby, this: HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let reference = edit(&this)?;
    let Some(parent) = reference.parent() else {
        return Err(err("cannot replace a node with no parent"));
    };
    guard_doc_child_order(
        Some(parent.node()),
        Some(reference.node()),
        Some(reference.node()),
        arg_node(&rb_other)?,
    )?;
    let (ins, adopt_from) = prepare_insert(reference, rb_other)?;
    splice_or_insert(reference, ins, Insert::Before, false);
    reference.detach();
    invalidate_indexes(this.document);
    inserted_result(rb_self, rb_other, ins, adopt_from)
}

/* ------------------------------------------------------------------ *
 * attribute and content mutation                                     *
 * ------------------------------------------------------------------ */

use crate::bridge::fragment::fragment_error;
use crate::bridge::string::{ruby_verified_data, ruby_verified_text, HtmlSource};
use crate::lexbor::adapter::html::{HtmlDoc, ScratchElement, NS_UNDEF};
use crate::lexbor::fragment::{Emit, TransientFragment};

/// `element[name] = value` -> value.
pub fn aset(_ruby: &Ruby, this: HtmlSelf, rb_name: Value, rb_value: Value) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Err(err("cannot set an attribute on a non-element node"));
    };
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    let vv = ruby_verified_data(rb_value, c"attribute value")?;
    /* SAFETY: both views are the caller's, live for this call, and Lexbor
     * copies them before any Ruby code can run again. */
    let stored = unsafe { el.set_attribute(nv.bytes(), vv.bytes()) };
    if stored.is_none() {
        return Err(err("failed to set attribute"));
    }
    invalidate_indexes(this.document);
    Ok(rb_value)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Err(err("cannot set an attribute on a non-element node"));
    };

    let qv = ruby_verified_text(rb_qname, c"attribute qualified name")?;
    let vv = ruby_verified_data(rb_value, c"attribute value")?;
    let nv = if rb_ns.is_nil() {
        None
    } else {
        Some(ruby_verified_text(rb_ns, c"namespace")?)
    };

    /* SAFETY: every view is the caller's, live for this call, and Lexbor copies
     * what it keeps before any Ruby code can run again. */
    let (qname, value) = unsafe { (qv.bytes(), vv.bytes()) };
    /* An empty URI is no namespace: it names the attribute the unprefixed way. */
    let ns = match &nv {
        Some(nv) if nv.len() != 0 => {
            // SAFETY: as above - the caller's view, live for this call, and
            // read before any Ruby code can run again.
            Some(unsafe { nv.bytes() })
        }
        _ => None,
    };

    /* Intern the wanted namespace so the existing attribute is matched on
     * (namespace, local name) - the DOM key - rather than on the qualified
     * name. */
    let doc = el.element().node().owner_document();
    // SAFETY: the element's own Document, live for this call.
    let want_ns =
        unsafe { HtmlDoc::from_raw(doc) }.map_or(NS_UNDEF, |d| d.intern_ns(ns.unwrap_or(&[])));

    let local = match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    };

    let stored = match el.element().find_attr_ns(want_ns, local) {
        Some(existing) => existing.set_value(value),
        None => el.append_attribute(ns, qname, value),
    };
    if !stored {
        return Err(err("failed to set namespaced attribute"));
    }

    invalidate_indexes(this.document);
    Ok(rb_value)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Ok(ruby.qnil().as_value());
    };
    let lv = ruby_verified_text(rb_local, c"attribute local name")?;

    let mut want_ns = NS_UNDEF;
    if !rb_ns.is_nil() {
        let nv = ruby_verified_text(rb_ns, c"namespace")?;
        if nv.len() != 0 {
            let doc = el.element().node().owner_document();
            // SAFETY: the element's own Document, and the view is live here.
            want_ns = unsafe { HtmlDoc::from_raw(doc) }
                .map_or(NS_UNDEF, |d| d.intern_ns(unsafe { nv.bytes() }));
        }
    }

    // SAFETY: the view is the caller's, live for this call.
    let found = el.element().find_attr_ns(want_ns, unsafe { lv.bytes() });
    if let Some(attr) = found {
        el.attr_remove(attr);
        invalidate_indexes(this.document);
    }
    Ok(ruby.qnil().as_value())
}

/// `element.name = new_name` -> new_name.
pub fn set_name(_ruby: &Ruby, this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let Some(el) = edit(&this)?.element_mut() else {
        return Err(err("name= is only supported on elements"));
    };
    let nv = ruby_verified_text(rb_name, c"element name")?;

    // SAFETY: the element's own Document, and the view is live for this call.
    let scratch =
        unsafe { ScratchElement::create(el.element().node().owner_document(), nv.bytes()) };
    let Some(scratch) = scratch else {
        return Err(err("failed to rename element"));
    };
    scratch.rename(el);
    invalidate_indexes(this.document);
    Ok(rb_name)
}

/// `node.content = text` -> text.
pub fn set_content(_ruby: &Ruby, this: HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    let node = edit(&this)?;
    let tv = ruby_verified_data(rb_text, c"node content")?;
    // SAFETY: the view is the caller's, live for this call.
    if !node.set_text_content(unsafe { tv.bytes() }) {
        return Err(err("failed to set node content"));
    }
    invalidate_indexes(this.document);
    Ok(rb_text)
}

/// `element.delete(name)` -> self.
pub fn delete(_ruby: &Ruby, this: HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let Some(el) = edit(&this)?.element_mut() else {
        return Ok(rb_self);
    };
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    // SAFETY: the view is the caller's, live for this call.
    unsafe { el.remove_attribute(nv.bytes()) };
    invalidate_indexes(this.document);
    Ok(rb_self)
}

/// Parse `rb_html` as a fragment in the context of `context_el`. Nothing is
/// changed yet: a String that fails to convert or parse leaves the tree as it
/// was, and the caller splices the result in with [`splice_fragment`].
unsafe fn parse_fragment_for(
    context_el: RawNode,
    rb_html: Value,
) -> Result<TransientFragment, Error> {
    /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
    let html = crate::bridge::ruby::string_of(rb_html)?.as_value();
    let src = HtmlSource::from_ruby(html)?;
    TransientFragment::parse(src.bytes(), src.known_valid(), context_el).map_err(fragment_error)
}

/// Import `frag`'s children into `doc`, placed by `emit`.
unsafe fn splice_fragment(frag: TransientFragment, doc: RawDoc, emit: Emit) -> Result<(), Error> {
    if !frag.import_into(doc, &emit) {
        return Err(err("failed to import a fragment child"));
    }
    Ok(())
}

/// `element.inner_html = html` -> html.
pub fn set_inner_html(_ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    let node = edit(&this)?;
    if node.node().node_type() != TYPE_ELEMENT {
        return Err(err("inner_html= requires an element"));
    }
    let context = RawNode::from(node.node());
    // SAFETY: `node` is a live element of this document.
    let frag = unsafe { parse_fragment_for(context, rb_html) }?;

    /* Only now that the input parsed: detach the existing children (the arena
     * reclaims them at document destroy) and put the new ones in. */
    while let Some(c) = node.first_child() {
        c.detach();
    }
    // SAFETY: `node` and its document are live for this call.
    unsafe {
        splice_fragment(
            frag,
            node.node().owner_document_handle(),
            Emit::Append(context),
        )
    }?;
    invalidate_indexes(this.document);
    Ok(rb_html)
}

/// `node.outer_html = html` -> html.
pub fn set_outer_html(_ruby: &Ruby, this: HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    let node = edit(&this)?;
    let parent = node.parent();
    if parent.is_none_or(|p| p.node().node_type() != TYPE_ELEMENT) {
        return Err(err("outer_html= requires a node with a parent element"));
    }
    let parent = parent.expect("checked just above");

    // SAFETY: `parent` and `node` are live nodes of this document.
    unsafe {
        let frag = parse_fragment_for(RawNode::from(parent.node()), rb_html)?;
        splice_fragment(
            frag,
            node.node().owner_document_handle(),
            Emit::Before(RawNode::from(node.node())),
        )?;
    }
    node.detach();
    invalidate_indexes(this.document);
    Ok(rb_html)
}

/* ------------------------------------------------------------------ *
 * node creation (Document)                                           *
 * ------------------------------------------------------------------ */

fn owning_doc(rb_self: &Value) -> Result<HtmlDoc<'_>, Error> {
    let doc = html_doc_unwrap(*rb_self)?;
    // SAFETY: a live HTML Document, kept alive by `rb_self` for this call.
    Ok(unsafe { doc.as_doc() })
}

/// `Document#create_element(name)` -> Element.
pub fn create_element(_ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let nv = ruby_verified_text(rb_name, c"element name")?;
    // SAFETY: the view is the caller's, live for this call.
    let Some(el) = doc.create_element(unsafe { nv.bytes() }) else {
        return Err(err("failed to create element"));
    };
    Ok(wrap_html_node(RawNode::from(el), rb_self))
}

/// `Document#create_text_node(content)` -> Text.
pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"text content")?;
    // SAFETY: the view is the caller's, live for this call.
    let Some(t) = doc.create_text(unsafe { tv.bytes() }) else {
        return Err(err("failed to create text node"));
    };
    Ok(wrap_html_node(RawNode::from(t), rb_self))
}

/// `Document#create_comment(content)` -> Comment.
pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"comment content")?;
    // SAFETY: the view is the caller's, live for this call.
    let Some(c) = doc.create_comment(unsafe { tv.bytes() }) else {
        return Err(err("failed to create comment"));
    };
    Ok(wrap_html_node(RawNode::from(c), rb_self))
}

/// `Document#create_processing_instruction(target, data)` -> PI.
pub fn create_pi(
    _ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_text(rb_target, c"processing instruction target")?;
    let dv = ruby_verified_text(rb_data, c"processing instruction data")?;
    // SAFETY: both views are the caller's, live for this call.
    let Some(pi) = doc.create_pi(unsafe { tv.bytes() }, unsafe { dv.bytes() }) else {
        return Err(err("failed to create processing instruction"));
    };
    Ok(wrap_html_node(RawNode::from(pi), rb_self))
}

/// `Document#create_document_type(name, public_id = "", system_id = "")`.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let args =
        magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
            args,
        )?;
    let (rb_name,) = args.required;
    let (rb_pub, rb_sys_) = args.optional;

    let doc = owning_doc(&rb_self)?;
    let nv = ruby_verified_text(rb_name, c"doctype name")?;
    // SAFETY: the view is the caller's, live for this call.
    let name = unsafe { nv.bytes() };
    if !HtmlDoc::valid_doctype_name(name) {
        /* The caller's error, not Lexbor's, so the exception class is picked
         * here - the check itself is the DOM layer's. */
        return Err(Error::new(
            ruby.exception_arg_error(),
            "invalid doctype name",
        ));
    }

    let verified =
        |v: Option<Value>, what: &'static core::ffi::CStr| match v.filter(|v| !v.is_nil()) {
            Some(v) => ruby_verified_text(v, what).map(Some),
            None => Ok(None),
        };
    let pv = verified(rb_pub, c"doctype public id")?;
    let sv = verified(rb_sys_, c"doctype system id")?;
    // SAFETY: both views are the caller's, live for this call.
    let (pub_id, sys_id) = unsafe {
        (
            pv.as_ref().map(|v| v.bytes()),
            sv.as_ref().map(|v| v.bytes()),
        )
    };

    let Some(dt) = doc.create_doctype(name, pub_id, sys_id) else {
        return Err(err("failed to create doctype"));
    };
    Ok(wrap_html_node(RawNode::from(dt), rb_self))
}

/// `Document#create_document_fragment` -> an EMPTY DocumentFragment.
pub fn create_document_fragment(_ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let Some(f) = doc.create_fragment() else {
        return Err(err("failed to create document fragment"));
    };
    Ok(wrap_html_node(RawNode::from(f), rb_self))
}
