//! The HTML Document: parsing one, its read-only accessors, and import/clone.
//! Fragments are `bridge::fragment`'s; the wrapper and its mutation gate are
//! `bridge::wrapper`'s.
//!
//! The arena work is `lexbor`'s; what lives here is the part that must touch
//! raw Ruby values and Lexbor handles together - copying the source out of a
//! Ruby String before the wrapper allocation, releasing the GVL for the parse,
//! and minting the wrapper. Keeping it here is what lets `glue/doc.rs` be
//! ordinary safe orchestration.
//!
//! # Parsing releases the GVL
//!
//! `parse_html` and everything under it is Ruby-free, and a freshly parsed
//! document is not yet shared, so nothing can race it. The source is copied into
//! a C buffer BEFORE the wrapper is allocated: allocating the wrapper is a GC
//! point, and the copy must not straddle one while holding a borrowed pointer
//! into a Ruby String's backing store.

#![allow(unsafe_code)]

use magnus::{prelude::*, Error, RString, Ruby, Value};

use crate::bridge::ruby::makiri_error;

use crate::bridge::html::import_copy;
use crate::bridge::html::{html_node_unwrap, wrap_html_node};
use crate::bridge::ruby::value;
use crate::bridge::string::HtmlSource;
use crate::bridge::wrapper::{
    ensure_document_mutable, html_doc, html_doc_unwrap, keepalive_document, node_repr, DocKind,
    DocumentShell, NodeRepr, DOC_TYPE,
};
use crate::bridge::xml::doc_of;
use crate::bridge::xml::xml_node_document;
use crate::bridge::xml::{unwrap as xml_node_id, xml_mut_check};
use crate::lexbor::adapter::cross_import::cross_xml_to_html;
use crate::lexbor::adapter::html::RawNode;
use crate::lexbor::adapter::post_parse::parse_html;

/* ------------------------------------------------------------------ *
 * parsing                                                            *
 * ------------------------------------------------------------------ */

/// `Document._parse(source)`: parse HTML, releasing the GVL, and wrap it.
///
/// The Ruby-level `Document.parse` coerces `source` to a String (and reads IO)
/// before calling this. Source locations for `Node#line` are always tracked.
pub fn parse_document(source: Value) -> Result<Value, Error> {
    let s = source.to_r_string()?;
    /* Honour the input's encoding: UTF-8/US-ASCII/binary pass through,
     * anything else is transcoded so its content survives. */
    let src = HtmlSource::from_ruby(s.as_value())?;

    /* Copy the source out BEFORE allocating the wrapper. Allocating is a GC
     * point, and a borrowed pointer into a Ruby String's backing store must
     * not straddle one - nor be held while the GVL is released. A source
     * already known to be valid UTF-8 lets the parse skip its sanitisation. */
    let assume_valid = src.known_valid();
    let owned = src.to_owned_bytes()?;
    drop(src);

    /* Allocate the wrapper with a null handle, so a failed parse still
     * frees cleanly through GC. This entry is defined on
     * Makiri::HTML::Document, so the result is always HTML. */
    let shell = DocumentShell::new(DocKind::Html);

    let result = crate::bridge::gvl::without_gvl(|| {
        // SAFETY: the bytes are `owned`'s, valid for the closure's lifetime,
        // and the parser only reads them.
        unsafe {
            parse_html(
                owned.as_slice().as_ptr(),
                owned.as_slice().len(),
                assume_valid,
            )
        }
        .map_or(core::ptr::null_mut(), Box::into_raw)
    });
    drop(owned);

    if result.is_null() {
        return Err(makiri_error("failed to parse HTML document"));
    }
    /* The GC learns the arena's size in `install`; `owned` is already gone, so
     * a collection that triggers has nothing of ours to invalidate. */
    // SAFETY: `result` is the handle the parse just returned, owned by no one.
    Ok(shell.install(unsafe { Box::from_raw(result) }))
}

/* ------------------------------------------------------------------ *
 * read-only accessors                                                *
 * ------------------------------------------------------------------ */

/// `Document#root`: the root Element node, or nil (unreachable today - the HTML
/// parser inserts html/head/body even for empty input).
pub fn document_root(ruby: &Ruby, rb_doc: Value) -> Value {
    let root = html_doc(&rb_doc).as_node().document_root();
    let Some(root) = root else {
        return ruby.qnil().as_value();
    };
    wrap_html_node(RawNode::from(root), rb_doc)
}

/// `Document#title`: the document `<title>`, or `""`.
pub fn document_title(ruby: &Ruby, rb_doc: Value) -> RString {
    let bytes = html_doc(&rb_doc).title().unwrap_or(&[]);
    ruby.enc_str_new(bytes, ruby.utf8_encoding())
}

/// `Document#quirks_mode`, as an Integer matching Lexbor (and Gumbo/Nokogiri):
/// 0 no-quirks, 1 quirks, 2 limited-quirks. Set by the parser from the doctype.
pub fn document_quirks_mode(ruby: &Ruby, rb_doc: Value) -> Value {
    let mode = html_doc(&rb_doc).compat_mode();
    ruby.integer_from_i64(mode).as_value()
}

/// `Document#errors`: the (currently always empty) parse-warning Array.
pub fn document_errors(rb_doc: Value) -> Value {
    /* A Document method, so the receiver is a Document. */
    let d: &crate::bridge::wrapper::DocData = DOC_TYPE.get_known(&rb_doc);
    // SAFETY: `d.errors` is the live Array the wrapper marks.
    unsafe { value(d.errors) }
}

/* ------------------------------------------------------------------ *
 * import and clone                                                   *
 * ------------------------------------------------------------------ */

/// `Document#import_node(node, deep = false)`: a copy of `node` owned by THIS
/// document - the DOM importNode, whose `deep` defaults to false.
///
/// Unlike `Node#clone_node` the copy belongs to the receiver, so this is the way
/// to bring a node across documents (Makiri never moves one between arenas). The
/// source is untouched and the copy is detached.
pub fn import_node(rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let (node_v,) = a.required;
    let deep = a.optional.0.map(|v| v.to_bool()).unwrap_or(false);

    let doc = html_doc_unwrap(rb_self)?;
    /* The copy is made in this document: refused while a handler reads it. */
    ensure_document_mutable(rb_self)?;

    /* An XML node is TRANSLATED across representations (mkr -> lxb) into a
     * detached lxb subtree owned by this document. */
    if node_repr(node_v) == NodeRepr::Xml {
        let mut imp = core::ptr::null_mut();
        let xdoc = doc_of(xml_node_document(node_v)?);
        let src = xml_node_id(node_v)?;
        // SAFETY: two live arenas, and the translation validates the target.
        xml_mut_check(unsafe {
            cross_xml_to_html(doc.as_ptr() as *mut _, xdoc, src, deep, &mut imp)
        })?;
        return Ok(wrap_html_node(
            RawNode::from_ptr(imp.cast()).expect("imported node"),
            rb_self,
        ));
    }

    let src = html_node_unwrap(node_v)?; /* Err on a non-node */
    // SAFETY: `src` is a live node; the copy is imported into `doc`.
    let imp = unsafe { import_copy(doc, src, deep, "import node") }?;
    Ok(wrap_html_node(imp, rb_self))
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
///
/// Built on the same import + `<template>`-content fixup as the fragment parser,
/// so a deep-cloned `<template>` carries its contents (which `import_node` alone
/// omits). Fails closed: a null import is an error rather than a partial node.
pub fn clone_node(rb_self: Value, args: &[Value]) -> Result<Value, Error> {
    /* The 0..1 arity by hand: `scan_args` cost about a third of a shallow
     * clone. The message is the one `rb_scan_args` gives. */
    let deep = match args {
        [] => false,
        /* RTEST: anything but nil and false. */
        [v] => v.to_bool(),
        _ => {
            return Err(Error::new(
                Ruby::get_with(rb_self).exception_arg_error(),
                format!(
                    "wrong number of arguments (given {}, expected 0..1)",
                    args.len()
                ),
            ))
        }
    };

    let node = html_node_unwrap(rb_self)?;
    let document = keepalive_document(rb_self)?;
    /* The copy is made in this document: refused while a handler reads it. */
    ensure_document_mutable(document)?;
    // SAFETY: the node of a live wrapper, which keeps its document alive.
    let doc = unsafe { node.as_node() }.owner_document_handle();

    // SAFETY: `node` belongs to `doc`, the document the copy is imported into.
    let clone = unsafe { import_copy(doc, node, deep, "clone node") }?;
    Ok(wrap_html_node(clone, document))
}
