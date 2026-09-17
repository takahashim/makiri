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
//! copied and released in `bridge::lexbor`, and the verb hands back the copy.
//! The release happens only AFTER the insert has gone through, so a refused
//! insert leaves the source document alone.
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
use super::wrap;
use crate::lexbor::adapter::html::{HtmlDoc, RawDoc, RawNode, ScratchElement, NS_UNDEF};
use crate::glue::abi::{error_class, html_doc_unwrap, ruby_verified_text};

pub use crate::bridge::string::ruby_verified_data;
use crate::lexbor::fragment::{
    import_transient_fragment_children, run_fragment_parser, Emit, FragmentContext,
};

/* ------------------------------------------------------------------ *
 * shared helpers                                                     *
 * ------------------------------------------------------------------ */

fn err(msg: &str) -> Error {
    Error::new(error_class(), msg.to_owned())
}

/* The structural mutators - insert/remove/replace, their adopt and fragment
 * rules, and the doctype-order guard - live in the Ruby <-> Lexbor seam
 * (`bridge::lexbor`); this module re-exports them for `init_mutate`. */
pub use crate::bridge::lexbor::{add_child, after, before, lshift, remove, replace};

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
    let Some(el) = crate::bridge::lexbor::edit(&this)?.element_mut() else {
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
    crate::bridge::lexbor::invalidate_indexes(this.document);
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
    let Some(el) = crate::bridge::lexbor::edit(&this)?.element_mut() else {
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
    crate::bridge::lexbor::invalidate_indexes(this.document);
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
    let Some(el) = crate::bridge::lexbor::edit(&this)?.element_mut() else {
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
        crate::bridge::lexbor::invalidate_indexes(this.document);
    }
    Ok(ruby.qnil().as_value())
}

/// `element.name = new_name` -> new_name.
///
/// Renames in place with identity preserved: create a throwaway element with the
/// new name so the document interns it, copy its name fields onto this node,
/// then discard it.
pub fn set_name(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let Some(el) = crate::bridge::lexbor::edit(&this)?.element_mut() else {
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
    crate::bridge::lexbor::invalidate_indexes(this.document);
    Ok(rb_name)
}

/// `node.content = text` -> text. The DOM textContent setter: for an element
/// this replaces all children with a single text node; for a character-data node
/// it sets the data.
pub fn set_content(_ruby: &Ruby, this: super::HtmlSelf, rb_text: Value) -> Result<Value, Error> {
    let node = crate::bridge::lexbor::edit(&this)?;
    let tv = ruby_verified_data(rb_text, c"node content")?;
    /* SAFETY: the view is the caller's, live for this call. */
    if !node.set_text_content(unsafe { tv.bytes() }) {
        return Err(err("failed to set node content"));
    }
    // SAFETY: `this.document` is the node's live Document.
    crate::bridge::lexbor::invalidate_indexes(this.document);
    Ok(rb_text)
}

/// `element.delete(name)` -> self. Removes the attribute if present.
pub fn delete(_ruby: &Ruby, this: super::HtmlSelf, rb_name: Value) -> Result<Value, Error> {
    let rb_self = this.value;
    let Some(el) = crate::bridge::lexbor::edit(&this)?.element_mut() else {
        return Ok(rb_self);
    };
    let nv = ruby_verified_text(rb_name, c"attribute name")?;
    /* SAFETY: the view is the caller's, live for this call. */
    unsafe { el.remove_attribute(nv.bytes()) };
    // SAFETY: `this.document` is the element's live Document.
    crate::bridge::lexbor::invalidate_indexes(this.document);
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
/// paths in `lexbor::fragment`.
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
        let node = crate::bridge::lexbor::edit(&this)?;
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
        crate::bridge::lexbor::invalidate_indexes(this.document);
        Ok(rb_html)
    }
}

/// `node.outer_html = html` -> html. Replaces the node itself with the parse.
pub fn set_outer_html(_ruby: &Ruby, this: super::HtmlSelf, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = crate::bridge::lexbor::edit(&this)?;
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
        crate::bridge::lexbor::invalidate_indexes(this.document);
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
    Ok(wrap(RawNode::from(el), rb_self))
}

pub fn create_text_node(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"text content")?;
    /* SAFETY: the view is the caller's, live for this call. */
    let Some(t) = doc.create_text(unsafe { tv.bytes() }) else {
        return Err(err("failed to create text node"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(wrap(RawNode::from(t), rb_self))
}

pub fn create_comment(_ruby: &Ruby, rb_self: Value, rb_text: Value) -> Result<Value, Error> {
    let doc = owning_doc(&rb_self)?;
    let tv = ruby_verified_data(rb_text, c"comment content")?;
    /* SAFETY: the view is the caller's, live for this call. */
    let Some(c) = doc.create_comment(unsafe { tv.bytes() }) else {
        return Err(err("failed to create comment"));
    };
    /* SAFETY: a fresh node of `rb_self`'s document, which keeps it alive. */
    Ok(wrap(RawNode::from(c), rb_self))
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
    Ok(wrap(RawNode::from(pi), rb_self))
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
    Ok(wrap(RawNode::from(dt), rb_self))
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
    Ok(wrap(RawNode::from(f), rb_self))
}
