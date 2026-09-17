//! The HTML node's mutators and the Document factories (glue/ruby_html_mutate.c).
//!
//! Thin wrappers over Lexbor's insert/remove/create, plus the safety checks
//! Lexbor itself omits: no cycles (a node cannot become a descendant of
//! itself), attribute nodes are not tree children, and the WHATWG doctype
//! ordering at the document node.
//!
//! # Adopt, never relink
//!
//! A node from another document is ADOPTED, as the DOM says appendChild does.
//! Lexbor's arenas own their nodes, so it cannot be relinked across them: it is
//! copied here ([`adopt_copy`]) and released there ([`adopt_release`]), and the
//! verb hands back the copy. The release happens only AFTER the insert has gone
//! through, so a refused insert leaves the source document alone.
//!
//! # Detach, never destroy
//!
//! The document arena owns all node memory and frees it wholesale, and live Ruby
//! wrappers may still point at a removed node, so `remove`/`unlink` only detach.
//!
//! # Every structural change drops the indexes
//!
//! The attr->owner + element-by-tag index and the text index are rebuilt on the
//! next query. [`invalidate`] is the one place that happens, and a mutator that
//! forgot to call it would serve a stale answer that looks entirely well-formed.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, Ruby, Value};

use super::ty;
use super::{node_document, unwrap, wrap};
use crate::lexbor::adapter::html::{
    check_document_child_order, DocumentChildOrderError, HtmlDoc, HtmlNode, HtmlNodeMut, RawDoc,
    RawNode, ScratchElement, NS_UNDEF,
};
use crate::glue::abi::{error_class, html_doc_unwrap, ruby_verified_text};

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

pub use crate::bridge::string::ruby_verified_data;
pub use crate::glue::fragment::html_import_deep;
pub use crate::glue::fragment::import_fragment_children;
pub use crate::glue::fragment::import_transient_fragment_children;
pub use crate::glue::fragment::run_fragment_parser;
pub use crate::glue::fragment::{Emit, FragmentContext};

/* ------------------------------------------------------------------ *
 * shared helpers                                                     *
 * ------------------------------------------------------------------ */

fn err(msg: &str) -> Error {
    Error::new(error_class(), msg.to_owned())
}

/// Drop the DOM and text indexes so the next query rebuilds them.
unsafe fn invalidate(document: Value) {
    if let Some(p) = crate::glue::doc::doc_parsed_known(document).as_mut() {
        p.invalidate_indexes();
    }
}

/// Every mutator unwraps `self` through here: a node the caller has frozen is
/// immutable, so raise FrozenError rather than silently editing it, and a
/// document an XPath handler is being evaluated over refuses to change. The
/// readers use [`unwrap`] directly.
fn unwrap_mutable(this: &super::HtmlSelf) -> Result<HtmlNodeMut<'_>, Error> {
    crate::bridge::ruby::check_frozen(this.value)?;
    crate::glue::doc::ensure_document_mutable(this.document)?;
    // SAFETY: the two checks above are exactly what the type asks for - the
    // receiver is not frozen, and no XPath evaluation is reading its document.
    Ok(unsafe { HtmlNodeMut::assume_mutable(this.node()) })
}

/// An HTML node argument. Routes through the HTML unwrap so an XML node is
/// rejected before its arena pointer reaches Lexbor.
fn arg_node(v: Value) -> Result<HtmlNode<'static>, Error> {
    let raw = unwrap(v)?;
    // SAFETY: `unwrap` checked `v` is an HTML node, and the caller holds `v`
    // for the length of the call, which keeps its document alive.
    Ok(unsafe { raw.as_node() })
}

/// Copy `node` into `doc`, for a node that came from another document - this
/// half of the DOM's adopt, or an error rather than a partial node.
unsafe fn adopt_copy(doc: RawDoc, node: HtmlNode<'_>) -> Result<HtmlNode<'static>, Error> {
    let imp = html_import_deep(doc, RawNode::from(node))?;
    // SAFETY: a node just imported into `doc`, which outlives this call.
    Ok(imp.as_node())
}

/// The other half: take `node` out of the document it came from, so the whole
/// thing reads as the move the DOM says appendChild performs.
fn adopt_release(node: HtmlNodeMut<'_>) {
    if node.node().node_type() == ty::FRAGMENT {
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
/// from any current parent (move semantics), and return the node to actually
/// insert.
///
/// For a node from another document that is its COPY, so the returned node is
/// not always the one passed in and the caller must insert - and hand back -
/// what this returns. The second element is the argument in that case, for
/// [`inserted_result`] to release once the insert has gone through; holding the
/// Ruby `Value` rather than the raw node also keeps the source document
/// reachable until then.
unsafe fn prepare_insert(
    reference: HtmlNodeMut<'_>,
    rb_incoming: Value,
) -> Result<(HtmlNodeMut<'static>, Option<Value>), Error> {
    let incoming = arg_node(rb_incoming)?;

    if incoming.node_type() == ty::ATTRIBUTE {
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
        crate::glue::doc::ensure_document_mutable(node_document(rb_incoming)?)?;
        let copy = adopt_copy(doc, incoming)?;
        // SAFETY: a copy this call just made in `reference`'s document, which
        // the caller cleared for editing.
        return Ok((HtmlNodeMut::assume_mutable(copy), Some(rb_incoming)));
    }
    // SAFETY: same document as `reference`, which the caller cleared.
    let incoming = HtmlNodeMut::assume_mutable(incoming);
    if incoming.parent().is_some() {
        incoming.detach();
    }
    Ok((incoming, None))
}

/// The value an insertion verb hands back: its argument, or - when the node was
/// adopted - the node now in the tree, which is a different object. Finishing
/// the adoption here keeps the release after the insert, where it belongs.
unsafe fn inserted_result(
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
            adopt_release(HtmlNodeMut::assume_mutable(arg_node(src)?));
            Ok(wrap(RawNode::from(inserted.node()), node_document(rb_self)?))
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
        DocumentChildOrderError::DoctypeAfterElement | DocumentChildOrderError::ElementBeforeDoctype => {
            err("a doctype must precede the document element")
        }
    })
}

/// Insert `node` relative to `anchor`, or - when `node` is a document fragment -
/// splice its children there in order, leaving the fragment empty.
///
/// With `advance` (insert_after semantics) each spliced child becomes the anchor
/// for the next, so document order is preserved; child/before splices keep a
/// fixed anchor. The one place the fragment-vs-single-node rule lives.
fn splice_or_insert<'d>(
    mut anchor: HtmlNodeMut<'d>,
    node: HtmlNodeMut<'d>,
    insert: Insert,
    advance: bool,
) {
    if node.node().node_type() != ty::FRAGMENT {
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
 * tree mutation                                                      *
 * ------------------------------------------------------------------ */

/// `node.add_child(child)` -> child. Appends as the last child; a document
/// fragment contributes its children rather than itself.
pub fn add_child(_ruby: &Ruby, this: super::HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let parent = unwrap_mutable(&this)?;
        guard_doc_child_order(Some(parent.node()), None, None, arg_node(rb_child)?)?;
        let (ins, adopt_from) = prepare_insert(parent, rb_child)?;
        splice_or_insert(parent, ins, Insert::Child, false);
        invalidate(this.document);
        inserted_result(rb_self, rb_child, ins, adopt_from)
    }
}

/// `node << child` -> node (chainable).
pub fn lshift(ruby: &Ruby, this: super::HtmlSelf, rb_child: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    add_child(ruby, this, rb_child)?;
    Ok(rb_self)
}

pub fn before(_ruby: &Ruby, this: super::HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let reference = unwrap_mutable(&this)?;
        let Some(parent) = reference.parent() else {
            return Err(err("cannot add a sibling to a node with no parent"));
        };
        guard_doc_child_order(
            Some(parent.node()),
            Some(reference.node()),
            None,
            arg_node(rb_node)?,
        )?;
        let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
        splice_or_insert(reference, ins, Insert::Before, false);
        invalidate(this.document);
        inserted_result(rb_self, rb_node, ins, adopt_from)
    }
}

pub fn after(_ruby: &Ruby, this: super::HtmlSelf, rb_node: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let reference = unwrap_mutable(&this)?;
        let Some(parent) = reference.parent() else {
            return Err(err("cannot add a sibling to a node with no parent"));
        };
        guard_doc_child_order(
            Some(parent.node()),
            reference.next().map(|n| n.node()),
            None,
            arg_node(rb_node)?,
        )?;
        let (ins, adopt_from) = prepare_insert(reference, rb_node)?;
        splice_or_insert(reference, ins, Insert::After, true);
        invalidate(this.document);
        inserted_result(rb_self, rb_node, ins, adopt_from)
    }
}

/// `node.remove` / `node.unlink` -> node. Detaches from the tree; the node stays
/// usable, because the arena owns it.
pub fn remove(_ruby: &Ruby, this: super::HtmlSelf) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let node = unwrap_mutable(&this)?;
        if node.node().node_type() == ty::ATTRIBUTE {
            return Err(err("use delete(name) to remove an attribute"));
        }
        if node.parent().is_some() {
            node.detach();
            invalidate(this.document);
        }
        Ok(rb_self)
    }
}

/// `node.replace(other)` -> other. Puts `other` where `node` is, detaches node.
pub fn replace(_ruby: &Ruby, this: super::HtmlSelf, rb_other: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    unsafe {
        let reference = unwrap_mutable(&this)?;
        let Some(parent) = reference.parent() else {
            return Err(err("cannot replace a node with no parent"));
        };
        guard_doc_child_order(
            Some(parent.node()),
            Some(reference.node()),
            Some(reference.node()),
            arg_node(rb_other)?,
        )?;
        let (ins, adopt_from) = prepare_insert(reference, rb_other)?;
        splice_or_insert(reference, ins, Insert::Before, false);
        reference.detach();
        invalidate(this.document);
        inserted_result(rb_self, rb_other, ins, adopt_from)
    }
}

/* ------------------------------------------------------------------ *
 * attribute mutation                                                 *
 * ------------------------------------------------------------------ */

/// `element[name] = value` -> value.
pub fn aset(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_name: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    let Some(el) = unwrap_mutable(&this)?.element_mut() else {
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
    // SAFETY: `this.document` is the element's live Document.
    unsafe { invalidate(this.document) };
    Ok(rb_value)
}

/// `element.set_attribute_ns(namespace_or_nil, qualified_name, value)` -> value.
///
/// Stores the attribute under its qualified name (case-preserved -
/// setAttributeNS is case-sensitive, unlike the HTML setAttribute family) and
/// records its OWN namespace on the attr node, so `namespaceURI` and
/// getAttributeNS resolve it. nil or `""` stores the null namespace.
pub fn set_attribute_ns(
    _ruby: &Ruby,
    this: super::HtmlSelf,
    rb_ns: Value,
    rb_qname: Value,
    rb_value: Value,
) -> Result<Value, Error> {
    let Some(el) = unwrap_mutable(&this)?.element_mut() else {
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
    /* An empty URI is no namespace: it names the attribute the unprefixed way,
     * which is a different Lexbor call rather than an empty argument. */
    let ns = match &nv {
        Some(nv) if nv.len() != 0 => Some(unsafe { nv.bytes() }),
        _ => None,
    };

    /* Intern the wanted namespace so the existing attribute is matched on
     * (namespace, local name) - the DOM key - rather than on the qualified
     * name. */
    let doc = el.element().node().owner_document();
    /* SAFETY: the element's own Document, live for this call. */
    let want_ns =
        unsafe { HtmlDoc::from_raw(doc) }.map_or(NS_UNDEF, |d| d.intern_ns(ns.unwrap_or(&[])));

    let local = match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    };

    /* A match keeps its qualified name (so re-setting with a different prefix
     * leaves the prefix unchanged); only the value updates. A miss appends a new
     * attribute, even when its qualified name collides with an existing one in a
     * different namespace. */
    let stored = match el.element().find_attr_ns(want_ns, local) {
        Some(existing) => existing.set_value(value),
        None => el.append_attribute(ns, qname, value),
    };
    if !stored {
        return Err(err("failed to set namespaced attribute"));
    }

    // SAFETY: `this.document` is the element's live Document.
    unsafe { invalidate(this.document) };
    Ok(rb_value)
}

/// `element.remove_attribute_ns(namespace_or_nil, local_name)` -> nil.
///
/// Removes the attribute matching (namespace, local name) - the DOM key - so a
/// namespaced attribute goes without disturbing a same-qualified-name one in
/// another namespace, which removal by qualified name would.
pub fn remove_attribute_ns(
    ruby: &Ruby,
    this: super::HtmlSelf,
    rb_ns: Value,
    rb_local: Value,
) -> Result<Value, Error> {
    let Some(el) = unwrap_mutable(&this)?.element_mut() else {
        return Ok(ruby.qnil().as_value());
    };
    let lv = ruby_verified_text(rb_local, c"attribute local name")?;

    let mut want_ns = NS_UNDEF;
    if !rb_ns.is_nil() {
        let nv = ruby_verified_text(rb_ns, c"namespace")?;
        if nv.len() != 0 {
            let doc = el.element().node().owner_document();
            /* SAFETY: the element's own Document, and the view is live here. */
            want_ns = unsafe { HtmlDoc::from_raw(doc) }
                .map_or(NS_UNDEF, |d| d.intern_ns(unsafe { nv.bytes() }));
        }
    }

    /* SAFETY: the view is the caller's, live for this call. */
    let found = el.element().find_attr_ns(want_ns, unsafe { lv.bytes() });
    if let Some(attr) = found {
        el.attr_remove(attr);
        // SAFETY: `this.document` is the element's live Document.
        unsafe { invalidate(this.document) };
    }
    Ok(ruby.qnil().as_value())
}

/// `element.name = new_name` -> new_name.
///
/// Renames in place with identity preserved: create a throwaway element with the
/// new name so the document interns it, copy its name fields onto this node,
/// then discard it.
pub fn set_name(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let Some(el) = unwrap_mutable(&this)?.element_mut() else {
        return Err(err("name= is only supported on elements"));
    };
    let nv = ruby_verified_text(rb_name, c"element name")?;

    /* SAFETY: the element's own Document, and the view is live for this call.
     * The scratch element is destroyed however this returns. */
    let scratch =
        unsafe { ScratchElement::create(el.element().node().owner_document(), nv.bytes()) };
    let Some(scratch) = scratch else {
        return Err(err("failed to rename element"));
    };
    scratch.rename(el);

    /* The element's tag id (local_name) is the key the element-by-tag index
     * buckets on and the //tag fast path serves from; renaming changes it, so a
     * persisted index would miss the element under its new name - a truncated,
     * wrong //newtag result. Drop the indexes like every other mutator. */
    // SAFETY: `this.document` is the element's live Document.
    unsafe { invalidate(this.document) };
    Ok(rb_name)
}

/// `node.content = text` -> text. The DOM textContent setter: for an element
/// this replaces all children with a single text node; for a character-data node
/// it sets the data.
pub fn set_content(_ruby: &Ruby, this: super::HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    let node = unwrap_mutable(&this)?;
    let tv = ruby_verified_data(rb_text, c"node content")?;
    /* SAFETY: the view is the caller's, live for this call. */
    if !node.set_text_content(unsafe { tv.bytes() }) {
        return Err(err("failed to set node content"));
    }
    // SAFETY: `this.document` is the node's live Document.
    unsafe { invalidate(this.document) };
    Ok(rb_text)
}

/// `element.delete(name)` -> self. Removes the attribute if present.
pub fn delete(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let Some(el) = unwrap_mutable(&this)?.element_mut() else {
        return Ok(rb_self);
    };
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    /* SAFETY: the view is the caller's, live for this call. */
    unsafe { el.remove_attribute(nv.bytes()) };
    // SAFETY: `this.document` is the element's live Document.
    unsafe { invalidate(this.document) };
    Ok(rb_self)
}

/* ------------------------------------------------------------------ *
 * inner_html= / outer_html=                                          *
 * ------------------------------------------------------------------ */

/// Parse `rb_html` as a fragment in the context of `context_el` and splice the
/// imported nodes via `emit`.
///
/// UTF-8 decoding (browser-compatible: invalid bytes become U+FFFD) and the
/// import + `<template>`-content fixup are shared with the DocumentFragment
/// paths in `glue::fragment`.
unsafe fn parse_fragment_into(
    context_el: RawNode,
    rb_html: Value,
    doc: RawDoc,
    emit: Emit,
) -> Result<(), Error> {
    /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
    let html = crate::bridge::ruby::string_of(rb_html)?.as_value();
    let frag = run_fragment_parser(html.as_raw(), &FragmentContext::Element(context_el))?;

    /* The fragment was built in a TRANSIENT document that destroying the parser
     * does NOT free (measured: one leaked per inner_html=/outer_html= call).
     * Owning it here frees it however this returns - the import below can fail,
     * and returning that error first used to skip the free. */
    let imported = import_transient_fragment_children(doc, frag, &emit);
    let _anchor = html;

    if !imported {
        return Err(err("failed to import a fragment child"));
    }
    Ok(())
}

/// `element.inner_html = html` -> html. Replaces the element's children.
pub fn set_inner_html(_ruby: &Ruby, this: super::HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(&this)?;
        if node.node().node_type() != ty::ELEMENT {
            return Err(err("inner_html= requires an element"));
        }

        /* Detach the existing children; the arena reclaims them at document
         * destroy. */
        while let Some(c) = node.first_child() {
            c.detach();
        }

        parse_fragment_into(
            RawNode::from(node.node()),
            rb_html,
            node.node().owner_document_handle(),
            Emit::Append(RawNode::from(node.node())),
        )?;
        invalidate(this.document);
        Ok(rb_html)
    }
}

/// `node.outer_html = html` -> html. Replaces the node itself with the parse.
pub fn set_outer_html(_ruby: &Ruby, this: super::HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = unwrap_mutable(&this)?;
        let parent = node.parent();
        if parent.is_none_or(|p| p.node().node_type() != ty::ELEMENT) {
            return Err(err("outer_html= requires a node with a parent element"));
        }
        let parent = parent.expect("checked just above");

        /* Parse in the parent's context, splice the imported nodes before self. */
        parse_fragment_into(
            RawNode::from(parent.node()),
            rb_html,
            node.node().owner_document_handle(),
            Emit::Before(RawNode::from(node.node())),
        )?;
        node.detach();
        invalidate(this.document);
        Ok(rb_html)
    }
}

/* ------------------------------------------------------------------ *
 * node creation (Document)                                           *
 * ------------------------------------------------------------------ */

/// The Lexbor document behind a Ruby Document, as a handle.
///
/// Taken by reference so the handle's lifetime is the borrow of `rb_self`: the
/// Document is what keeps the Lexbor document alive, and the type now says so.
/// `'static` would compile and would be a stronger claim than the truth.
fn owning_doc(rb_self: &Value) -> Result<HtmlDoc<'_>, Error> {
    let doc = html_doc_unwrap(*rb_self)?;
    // SAFETY: a live HTML Document, kept alive by `rb_self` for this call.
    Ok(unsafe { doc.as_doc() })
}

pub fn create_element(_ruby: &Ruby, rb_self: Value, rb_name: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let nv = ruby_verified_text(rb_name, c"element name")?;
    /* SAFETY: the view is the caller's, live for this call. */
    let Some(el) = doc.create_element(unsafe { nv.bytes() }) else {
        return Err(err("failed to create element"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(unsafe { wrap(RawNode::from(el), rb_self) })
}

pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"text content")?;
    /* SAFETY: the view is the caller's, live for this call. */
    let Some(t) = doc.create_text(unsafe { tv.bytes() }) else {
        return Err(err("failed to create text node"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(unsafe { wrap(RawNode::from(t), rb_self) })
}

pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"comment content")?;
    /* SAFETY: the view is the caller's, live for this call. */
    let Some(c) = doc.create_comment(unsafe { tv.bytes() }) else {
        return Err(err("failed to create comment"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(unsafe { wrap(RawNode::from(c), rb_self) })
}

/// `Document#create_processing_instruction(target, data)` - the DOM
/// createProcessingInstruction: a detached PI owned by this document. Lexbor
/// validates the target, so an invalid one fails closed.
pub fn create_pi(
    _ruby: &Ruby,
    rb_self: Value,
    rb_target: Value,
    rb_data: Value,
) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_text(rb_target, c"processing instruction target")?;
    let dv = ruby_verified_text(rb_data, c"processing instruction data")?;
    /* SAFETY: both views are the caller's, live for this call. */
    let Some(pi) = doc.create_pi(unsafe { tv.bytes() }, unsafe { dv.bytes() }) else {
        return Err(err("failed to create processing instruction"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(unsafe { wrap(RawNode::from(pi), rb_self) })
}

/// `Document#create_document_type(name, public_id = "", system_id = "")` - the
/// DOM DOMImplementation.createDocumentType: a detached DocumentType owned by
/// this document, to be placed before the document element (the tree guards
/// enforce that). An empty or omitted public/system id is treated as absent.
/// Lexbor validates the name as a DOM Name, so an invalid one fails closed.
pub fn create_document_type(ruby: &Ruby, rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let args =
        magnus::scan_args::scan_args::<(Value,), (Option<Value>, Option<Value>), (), (), (), ()>(
            args,
        )?;
    let (rb_name,) = args.required;
    let (rb_pub, rb_sys_) = args.optional;

    let doc = owning_doc(&rb_self)?;
    let nv = ruby_verified_text(rb_name, c"doctype name")?;
    /* SAFETY: the view is the caller's, live for this call. */
    let name = unsafe { nv.bytes() };
    if !HtmlDoc::valid_doctype_name(name) {
        /* The caller's error, not Lexbor's, so the exception class is picked
         * here - the check itself is the DOM layer's. */
        return Err(Error::new(
            ruby.exception_arg_error(),
            "invalid doctype name",
        ));
    }

    /* An omitted id and an empty one are the same thing to the DOM: both report
     * nil. `None` is what reaches Lexbor as a null pointer. */
    let verified =
        |v: Option<Value>, what: &'static core::ffi::CStr| match v.filter(|v| !v.is_nil()) {
            Some(v) => ruby_verified_text(v, what).map(Some),
            None => Ok(None),
        };
    let pv = verified(rb_pub, c"doctype public id")?;
    let sv = verified(rb_sys_, c"doctype system id")?;
    /* SAFETY: both views are the caller's, live for this call. */
    let (pub_id, sys_id) = unsafe {
        (
            pv.as_ref().map(|v| v.bytes()),
            sv.as_ref().map(|v| v.bytes()),
        )
    };

    let Some(dt) = doc.create_doctype(name, pub_id, sys_id) else {
        return Err(err("failed to create doctype"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(unsafe { wrap(RawNode::from(dt), rb_self) })
}

/// `Document#create_document_fragment` - the DOM createDocumentFragment: an
/// EMPTY DocumentFragment owned by this document, unlike `#fragment` /
/// `DocumentFragment.parse`, which parse HTML.
pub fn create_document_fragment(_ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let Some(f) = doc.create_fragment() else {
        return Err(err("failed to create document fragment"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(unsafe { wrap(RawNode::from(f), rb_self) })
}
