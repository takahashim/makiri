//! The Ruby <-> document seam: parsing a Document, its read-only accessors, the
//! fragment pipeline, and cross-document import/clone.
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

use core::ffi::c_int;

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, RString, Ruby, Value};

use crate::bridge::lexbor::{
    doc_of, html_doc_known, html_doc_unwrap, html_node_unwrap, keepalive_document, new_document,
    node_kind, set_document_parsed, wrap_document, wrap_html_node, DOC_TYPE,
};
use crate::bridge::ruby::{typed_data_known_ref, value};
use crate::bridge::string::{
    ruby_str_known_valid_utf8_value, ruby_string_bytes, ruby_to_utf8_value,
};
use crate::bridge::xml::{node_document as xml_node_document, unwrap as xml_node_id, xml_mut_check};
use crate::init::EXC_ERROR;
use crate::lexbor::adapter::cross_import::cross_xml_to_html;
use crate::lexbor::adapter::html::RawNode;
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::fragment::{
    build_fragment_ctx, context_kwarg, import_with_fixup, resolve_fragment_context,
};

/// Generated, not transcribed. A hand-written 1 here (it is 2) made
/// `import_node` treat every HTML node as an XML one.
const NODE_KIND_XML: c_int = crate::lexbor::ffi::NODE_KIND_XML as c_int;

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
    let src = ruby_to_utf8_value(s.as_value());

    /* Copy the source out BEFORE allocating the wrapper. Allocating is a GC
     * point, and a borrowed pointer into a Ruby String's backing store must
     * not straddle one - nor be held while the GVL is released. The
     * coderange is read first (no scan): a source Ruby already knows is
     * valid UTF-8 lets the parse skip its sanitisation. */
    let assume_valid = ruby_str_known_valid_utf8_value(src);
    let owned = ruby_string_bytes(src)?;

    /* Allocate the wrapper with a null handle, so a failed parse still
     * frees cleanly through GC. This entry is defined on
     * Makiri::HTML::Document, so the result is always HTML. */
    let obj = new_document(true);

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
        return Err(Error::new(
            EXC_ERROR.exception(),
            "failed to parse HTML document",
        ));
    }
    set_document_parsed(obj, result);
    // SAFETY: `obj` came from `new_document`, which returns a live Document.
    Ok(unsafe { value(obj) })
}

/* ------------------------------------------------------------------ *
 * read-only accessors                                                *
 * ------------------------------------------------------------------ */

/// `Document#root`: the root Element node, or nil (unreachable today - the HTML
/// parser inserts html/head/body even for empty input).
pub fn document_root(ruby: &Ruby, rb_doc: Value) -> Value {
    // SAFETY: a live HTML Document, kept alive by `rb_doc` for this call.
    let root = unsafe { html_doc_known(rb_doc).as_doc() }
        .as_node()
        .document_root();
    let Some(root) = root else {
        return ruby.qnil().as_value();
    };
    // SAFETY: a node of `rb_doc`'s document, which keeps it alive.
    wrap_html_node(RawNode::from(root), rb_doc)
}

/// `Document#title`: the document `<title>`, or `""`.
pub fn document_title(ruby: &Ruby, rb_doc: Value) -> RString {
    // SAFETY: a live HTML Document, kept alive by `rb_doc` for this call.
    let bytes = unsafe { html_doc_known(rb_doc).as_doc() }
        .title()
        .unwrap_or(&[]);
    ruby.enc_str_new(bytes, ruby.utf8_encoding())
}

/// `Document#quirks_mode`, as an Integer matching Lexbor (and Gumbo/Nokogiri):
/// 0 no-quirks, 1 quirks, 2 limited-quirks. Set by the parser from the doctype.
pub fn document_quirks_mode(ruby: &Ruby, rb_doc: Value) -> Value {
    // SAFETY: a live HTML Document, kept alive by `rb_doc` for this call.
    let doc = unsafe { html_doc_known(rb_doc).as_doc().as_raw() };
    // SAFETY: `doc` is live, as above.
    ruby.integer_from_i64(unsafe { (*doc).compat_mode as i64 })
        .as_value()
}

/// `Document#errors`: the (currently always empty) parse-warning Array.
pub fn document_errors(rb_doc: Value) -> Value {
    /* A Document method, so the receiver is a Document. */
    let d: &crate::bridge::lexbor::DocData = typed_data_known_ref(rb_doc, &DOC_TYPE);
    // SAFETY: `d.errors` is the live Array the wrapper marks.
    unsafe { value(d.errors) }
}

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

/// The standalone fragment's backing document: a throwaway
/// `<html><body></body></html>` shell, owned by the fragment's wrapper.
pub fn fragment_shell_document() -> Result<Value, Error> {
    const SHELL: &[u8] = b"<html><body></body></html>";
    // SAFETY: a static byte string; the parse copies what it needs.
    let Some(parsed) = (unsafe { parse_html(SHELL.as_ptr(), SHELL.len(), true) }) else {
        return Err(Error::new(
            EXC_ERROR.exception(),
            "failed to create fragment document",
        ));
    };
    // SAFETY: the wrapper takes ownership of `parsed`; GC frees it.
    Ok(unsafe { value(wrap_document(Box::into_raw(parsed))) })
}

/// The body the fragment entry points share: read `(html, context:)`, resolve
/// the context against a document, and build the fragment in it.
///
/// The entry points differ in ONE thing - which document the fragment belongs
/// to - so that is what `make_document` supplies.
pub fn fragment_in(
    ruby: &Ruby,
    args: &[Value],
    make_document: impl FnOnce() -> Result<Value, Error>,
) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let (html,) = a.required;
    let context = context_kwarg(ruby, Some(a.keywords));
    let document = make_document()?;
    let doc = html_doc_unwrap(document)?;
    // SAFETY: `doc` is `document`'s live Lexbor document for this call.
    let (tag, ns) = unsafe { resolve_fragment_context(doc, context)? };
    // SAFETY: as above; the fragment is bound to `document`.
    unsafe { build_fragment_ctx(ruby, document, doc, html, tag, ns) }
}

/// Parse `html` as a fragment in the context named by `(tag, ns)` and return the
/// fragment, owned by `document`.
pub fn build_fragment(
    ruby: &Ruby,
    document: Value,
    html: Value,
    tag: usize,
    ns: usize,
) -> Result<Value, Error> {
    let doc = html_doc_unwrap(document)?;
    // SAFETY: `doc` is `document`'s live document; the fragment is bound to it.
    unsafe { build_fragment_ctx(ruby, document, doc, html, tag, ns) }
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

    /* An XML node is TRANSLATED across representations (mkr -> lxb) into a
     * detached lxb subtree owned by this document. */
    if node_kind(node_v.as_raw()) == NODE_KIND_XML {
        let mut imp = core::ptr::null_mut();
        let xdoc = doc_of(xml_node_document(node_v)?);
        let src = xml_node_id(node_v)?;
        // SAFETY: two live arenas, and the translation validates the target.
        xml_mut_check(unsafe { cross_xml_to_html(doc.as_ptr() as *mut _, xdoc, src, deep, &mut imp) })?;
        return Ok(wrap_html_node(
            RawNode::from_ptr(imp.cast()).expect("imported node"),
            rb_self,
        ));
    }

    let src = html_node_unwrap(node_v)?; /* Err on a non-node */
    // SAFETY: `src` is a live node; the copy is imported into `doc`.
    let Some(imp) = (unsafe { import_with_fixup(doc, src, deep) }) else {
        return Err(Error::new(EXC_ERROR.exception(), "failed to import node"));
    };
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
    // SAFETY: the node of a live wrapper, which keeps its document alive.
    let doc = unsafe { node.as_node() }.owner_document_handle();

    // SAFETY: `node` belongs to `doc`, the document the copy is imported into.
    let Some(clone) = (unsafe { import_with_fixup(doc, node, deep) }) else {
        return Err(Error::new(EXC_ERROR.exception(), "failed to clone node"));
    };
    let document = keepalive_document(rb_self)?;
    // SAFETY: `clone` is a detached node of `document`'s arena.
    Ok(wrap_html_node(clone, document))
}
