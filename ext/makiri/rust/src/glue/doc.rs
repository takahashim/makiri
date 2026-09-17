//! `Makiri::HTML::Document` and the fragment machinery (glue/ruby_doc.c).
//!
//!   `Document._parse(source)`, `#root`, `#title`, `#errors`,
//!   `#internal_subset`, `#quirks_mode`, `#fragment(html, context:)`,
//!   `#import_node(node, deep = false)`
//!   `DocumentFragment.parse(html, context:)`, `Node#parse(html)`
//!
//! # The Document wrapper, and what other glue modules share
//!
//! Ten methods, plus the pieces the rest of the glue uses: the Document wrapper
//! type and its `rb_data_type_t` chain, the parsed-handle accessors, and the
//! fragment pipeline `html_node::mutate` and `dom_adapter::cross_import` run
//! through. `glue::abi`'s compile-time check pins their signatures.
//!
//! # Parsing releases the GVL
//!
//! `parse_html` and everything under it is Ruby-free, and a freshly parsed
//! document is not yet shared, so nothing can race it. The source is copied into
//! a C buffer BEFORE the wrapper is allocated: allocating the wrapper is a GC
//! point, and the copy must not straddle one while holding a borrowed pointer
//! into a Ruby String's backing store.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, Error, RString, Ruby, Value};
use rb_sys::{rb_data_type_t, VALUE};

use super::abi::{
    error_class, html_node_unwrap, keepalive_document, ruby_copy_bytes, ruby_str_known_valid_utf8,
    ruby_to_utf8, wrap_html_node, xml_node_unwrap, DataType,
};
use crate::lexbor::fragment::{
    build_fragment_ctx, context_kwarg, import_with_fixup, resolve_fragment_context,
};
use crate::init::{
    CLASS_DOCUMENT_FRAGMENT, CLASS_HTML_DOCUMENT, CLASS_XML_DOCUMENT, MOD_HTML_NODE_METHODS,
};

/* ------------------------------------------------------------------ *
 * the wrapper                                                        *
 * ------------------------------------------------------------------ */

/// `mkr_doc_data_t`: the parsed handle (owned - GC frees it) and the reserved
/// errors Array.
struct DocData {
    parsed: *mut crate::lexbor::adapter::post_parse::Parsed,
    errors: VALUE,
}

/// Generated, not transcribed. A hand-written 1 here (it is 2) made
/// `import_node` treat every HTML node as an XML one.
const NODE_KIND_XML: c_int = crate::lexbor::ffi::NODE_KIND_XML as c_int;

use crate::lexbor::adapter::html::{RawDoc, RawNode};

pub use crate::lexbor::adapter::cross_import::cross_xml_to_html;
pub use crate::lexbor::adapter::post_parse::parse_html;
pub use crate::glue::node::node_kind;
pub use crate::glue::xml_node::mutate::xml_mut_check;
pub use crate::xml::api::xml_doc_memsize;

/// The doctype node type, generated (see lexbor_abi).
const NODE_TYPE_DOCUMENT_TYPE: u32 = super::abi::LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;

unsafe extern "C" fn doc_mark(ptr: *mut c_void) {
    let d = &*(ptr as *const DocData);
    rb_sys::rb_gc_mark(d.errors);
}

unsafe extern "C" fn doc_free(ptr: *mut c_void) {
    let d = ptr as *mut DocData;
    if !(*d).parsed.is_null() {
        drop(Box::from_raw((*d).parsed));
    }
    rb_sys::ruby_xfree(ptr);
}

unsafe extern "C" fn doc_memsize(ptr: *const c_void) -> rb_sys::size_t {
    let d = &*(ptr as *const DocData);
    let mut total = core::mem::size_of::<DocData>();
    // Lexbor's arena size is not cheaply queryable, so an HTML document reports
    // the wrapper only; the XML arena tracks its own byte total.
    if let Some(xdoc) = d.parsed.as_ref().and_then(|p| p.xml_doc_ref()) {
        total += xml_doc_memsize(xdoc);
    }
    total as rb_sys::size_t
}

const fn doc_data_type(name: *const c_char, parent: *const rb_data_type_t) -> DataType {
    DataType::new(
        name,
        parent,
        Some(doc_mark),
        Some(doc_free),
        Some(doc_memsize),
    )
}

/// The base type, exported: the kind-agnostic accessors (`doc_parsed`,
/// `#errors`) legitimately accept either representation.
pub static DOC_TYPE: DataType = doc_data_type(c"Makiri::Document".as_ptr(), core::ptr::null());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as a Lexbor document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
static HTML_DOC_TYPE: DataType =
    doc_data_type(c"Makiri::HTML::Document".as_ptr(), DOC_TYPE.as_ptr());
static XML_DOC_TYPE: DataType = doc_data_type(c"Makiri::XML::Document".as_ptr(), DOC_TYPE.as_ptr());

/// The Lexbor document behind an HTML Document. `Err(TypeError)` otherwise.
pub fn html_doc_unwrap(rb_doc: Value) -> Result<RawDoc, Error> {
    let d = crate::bridge::ruby::typed_data(rb_doc, &HTML_DOC_TYPE)? as *mut DocData;
    Ok(html_doc_of(d))
}

/// [`html_doc_unwrap`] for a VALUE already known to be an HTML Document.
pub fn html_doc_known(rb_doc: Value) -> RawDoc {
    html_doc_of(crate::bridge::ruby::typed_data_known(rb_doc, &HTML_DOC_TYPE) as *mut DocData)
}

fn html_doc_of(d: *mut DocData) -> RawDoc {
    /* An lxb_html_document_t leads with its lxb_dom_document_t, so this is a
     * downcast to the embedded base, not a reinterpretation. */
    // SAFETY: `d` is the data of a live HTML Document, whose handle it owns.
    unsafe { RawDoc::from_ptr((*(*d).parsed).html_doc().cast()).expect("live document") }
}

/// The parsed handle behind any Document. `Err(TypeError)` for a non-Document.
pub fn doc_parsed(rb_doc: Value) -> Result<*mut crate::lexbor::adapter::post_parse::Parsed, Error> {
    let d = crate::bridge::ruby::typed_data(rb_doc, &DOC_TYPE)? as *mut DocData;
    // SAFETY: the data of a live Document.
    Ok(unsafe { (*d).parsed })
}

/// Marks a document as read by an XPath evaluation that can run Ruby - one with
/// a handler - for as long as it lives. Nested evaluations stack.
///
/// The engine borrows names, attribute values and index slices out of the
/// document for the whole walk, and a handler runs arbitrary Ruby in the middle
/// of it. Lexbor frees an attribute's old value when a new one is set
/// (`lxb_dom_attr_set_value`), and a mutation drops the indexes, so a handler
/// that edited the same document could leave the evaluator reading freed
/// memory. Every mutator checks [`ensure_document_mutable`] first, so that
/// borrow is never invalidated under a suspended walk.
pub(crate) struct DocumentEvaluation(
    *mut crate::lexbor::adapter::post_parse::Parsed,
    /// The Document the handle belongs to. Holding it is what keeps the handle
    /// valid: a guard lives on the machine stack, which Ruby's collector scans,
    /// so the Document cannot be collected while one is alive.
    Value,
);

impl DocumentEvaluation {
    pub(crate) fn enter(rb_doc: Value) -> Result<Self, Error> {
        let p = doc_parsed(rb_doc)?;
        // SAFETY: the handle of a live Document, kept alive by the guard itself.
        unsafe { (*p).evaluating += 1 };
        Ok(DocumentEvaluation(p, rb_doc))
    }
}

impl Drop for DocumentEvaluation {
    fn drop(&mut self) {
        // SAFETY: `enter` counted this handle, and field 1 has kept its Document
        // - and so the handle - alive for as long as this guard.
        unsafe { (*self.0).evaluating -= 1 }
        /* Read the Document here, so the guard demonstrably holds it: the field
         * is there to keep it reachable, and a field nothing reads is one the
         * compiler is free to treat as absent. */
        core::hint::black_box(self.1);
    }
}

/// `Err(Makiri::Error)` while an evaluation with a handler is reading
/// `rb_doc`, a Document. Every mutator calls this before it changes anything.
pub fn ensure_document_mutable(rb_doc: Value) -> Result<(), Error> {
    // SAFETY: the handle of a live Document.
    if unsafe { (*doc_parsed_known(rb_doc)).evaluating } != 0 {
        return Err(Error::new(
            error_class(),
            "cannot modify a document while evaluating XPath over it (re-entrant mutation from a handler)",
        ));
    }
    Ok(())
}

/// [`doc_parsed`] for a VALUE already known to be a Document - a node's
/// keepalive Document, or the receiver of a Document method.
pub fn doc_parsed_known(rb_doc: Value) -> *mut crate::lexbor::adapter::post_parse::Parsed {
    let d = crate::bridge::ruby::typed_data_known(rb_doc, &DOC_TYPE) as *mut DocData;
    // SAFETY: the data of a live Document.
    unsafe { (*d).parsed }
}

/// Wrap an owned handle as a Document; GC takes ownership. The leaf class is
/// chosen by kind - a Lexbor-backed handle is an HTML Document, an arena-backed
/// one an XML Document.
pub unsafe extern "C" fn wrap_document(
    parsed: *mut crate::lexbor::adapter::post_parse::Parsed,
) -> VALUE {
    let is_xml = (*parsed).is_xml();
    let (klass, ty) = if is_xml {
        (CLASS_XML_DOCUMENT.raw(), XML_DOC_TYPE.as_ptr())
    } else {
        (CLASS_HTML_DOCUMENT.raw(), HTML_DOC_TYPE.as_ptr())
    };
    /* The errors array is created AFTER the wrap. Created before, it would sit
     * in this malloc'd struct - seen by no mark - across the wrap's allocation,
     * and a GC there frees it; `doc_mark` then marks a dead slot ("try to mark
     * T_NONE object" under GC_COMPACT_STRESS). */
    crate::bridge::ruby::wrap_zeroed::<DocData>(
        klass,
        ty,
        |d| d.parsed = parsed,
        |d| d.errors = crate::bridge::ruby::array_new().as_raw(),
    )
}

/* ---- Document.parse ---- */

/// Arguments for the GVL-released parse.
struct ParseArgs<'a> {
    src: &'a [u8],
    assume_valid: bool,
    result: *mut crate::lexbor::adapter::post_parse::Parsed,
}

/// Runs with the GVL released: pure C (Lexbor + libc), touching no Ruby state.
unsafe extern "C" fn parse_nogvl(p: *mut c_void) -> *mut c_void {
    let a = &mut *(p as *mut ParseArgs<'_>);
    a.result = parse_html(a.src.as_ptr(), a.src.len(), a.assume_valid)
        .map_or(core::ptr::null_mut(), Box::into_raw);
    core::ptr::null_mut()
}

/// `Document._parse(source)`. The Ruby-level `Document.parse` coerces `source`
/// to a String (and reads IO) before calling this. Source locations for
/// `Node#line` are always tracked.
fn doc_s_parse(ruby: &Ruby, klass: Value, source: Value) -> Result<Value, Error> {
    unsafe {
        let s = source.to_r_string()?;
        /* Honour the input's encoding: UTF-8/US-ASCII/binary pass through,
         * anything else is transcoded so its content survives. */
        let src = ruby_to_utf8(s.as_raw());

        /* Copy the source out BEFORE allocating the wrapper. Allocating is a GC
         * point, and a borrowed pointer into a Ruby String's backing store must
         * not straddle one - nor be held while the GVL is released. The
         * coderange is read first (no scan): a source Ruby already knows is
         * valid UTF-8 lets the parse skip its sanitisation. */
        let assume_valid = ruby_str_known_valid_utf8(src);
        let Some(owned) = ruby_copy_bytes(src) else {
            return Err(Error::new(error_class(), "out of memory copying source"));
        };

        /* Allocate the wrapper with a null handle, so a failed parse still
         * frees cleanly through GC. This entry is defined on
         * Makiri::HTML::Document, so the result is always HTML. */
        let mut d: *mut DocData = core::ptr::null_mut();
        /* The errors array comes after the wrap, as in `wrap_document`. */
        let obj = crate::bridge::ruby::wrap_zeroed::<DocData>(
            klass.as_raw(),
            HTML_DOC_TYPE.as_ptr(),
            |data| data.parsed = core::ptr::null_mut(),
            |data| {
                data.errors = crate::bridge::ruby::array_new().as_raw();
                d = data;
            },
        );

        let mut args = ParseArgs {
            src: owned.as_slice(),
            assume_valid,
            result: core::ptr::null_mut(),
        };
        rb_sys::rb_thread_call_without_gvl(
            Some(parse_nogvl),
            &mut args as *mut ParseArgs<'_> as *mut c_void,
            None,
            core::ptr::null_mut(),
        );
        let result = args.result;
        drop(owned);

        (*d).parsed = result;
        if (*d).parsed.is_null() {
            return Err(Error::new(error_class(), "failed to parse HTML document"));
        }
        let _ = ruby;
        Ok(Value::from_raw(obj))
    }
}

/* ---- read-only accessors ---- */

fn doc_root(ruby: &Ruby, self_: Value) -> Value {
    /* SAFETY: a live HTML Document, kept alive by `self_` for this call. */
    let root = unsafe { html_doc_known(self_).as_doc() }
        .as_node()
        .document_root();
    let Some(root) = root else {
        /* The HTML parser inserts html/head/body even for empty input, so this
         * is unreachable today. Returning nil rather than wrapping a null is
         * what the reachable behaviour would want if that ever changed. */
        return ruby.qnil().as_value();
    };
    /* SAFETY: a node of `self_`'s document, which keeps it alive. */
    unsafe { Value::from_raw(wrap_html_node(RawNode::from(root), self_.as_raw())) }
}

/// The document `<title>`, or `""`.
fn doc_title(ruby: &Ruby, self_: Value) -> RString {
    /* SAFETY: a live HTML Document, kept alive by `self_` for this call. */
    let bytes = unsafe { html_doc_known(self_).as_doc() }
        .title()
        .unwrap_or(&[]);
    ruby.enc_str_new(bytes, ruby.utf8_encoding())
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(_ruby: &Ruby, self_: Value) -> Result<Value, Error> {
    let doc = crate::glue::html_node::arg_node(&self_)?;
    let doctype = doc
        .children()
        .find(|c| c.node_type() == NODE_TYPE_DOCUMENT_TYPE);
    // SAFETY: the doctype is a child of this Document, its own keepalive.
    Ok(unsafe { crate::glue::html_node::wrap_node(doctype, self_) })
}

/// The quirks mode as an Integer matching Lexbor (and Gumbo/Nokogiri):
/// 0 no-quirks, 1 quirks, 2 limited-quirks. Set by the parser from the doctype.
fn doc_quirks_mode(ruby: &Ruby, self_: Value) -> Value {
    let _ = ruby;
    unsafe {
        let doc = html_doc_known(self_).as_doc().as_raw();
        ruby.integer_from_i64((*doc).compat_mode as i64).as_value()
    }
}

/// Parse warnings. Reserved; currently always empty.
fn doc_errors(ruby: &Ruby, self_: Value) -> Value {
    let _ = ruby;
    unsafe {
        /* A Document method, so the receiver is a Document. */
        let d = crate::bridge::ruby::typed_data_known(self_, &DOC_TYPE) as *mut DocData;
        Value::from_raw((*d).errors)
    }
}

/* ---- fragment entry points ---- */

/// `document.fragment(html, context: ...)` -> a DocumentFragment bound to this
/// document. `context` defaults to `<body>`.
fn doc_fragment(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    fragment_in(ruby, args, |_| Ok(self_))
}

/// `DocumentFragment.parse(html, context: ...)` -> a standalone fragment with
/// its own backing document, kept alive by the fragment's wrapper.
fn frag_s_parse(ruby: &Ruby, _klass: Value, args: &[Value]) -> Result<Value, Error> {
    fragment_in(ruby, args, |_| unsafe {
        const SHELL: &[u8] = b"<html><body></body></html>";
        let Some(parsed) = parse_html(SHELL.as_ptr(), SHELL.len(), true) else {
            return Err(Error::new(
                error_class(),
                "failed to create fragment document",
            ));
        };
        Ok(Value::from_raw(wrap_document(Box::into_raw(parsed)))) /* GC owns parsed now */
    })
}

/// The body both fragment entry points share: read `(html, context:)`, resolve
/// the context against a document, and build the fragment in it.
///
/// The two differ in ONE thing - which document the fragment belongs to - so
/// that is what the closure supplies. Written out twice, the shared four steps
/// were the kind of duplication that drifts.
fn fragment_in(
    ruby: &Ruby,
    args: &[Value],
    document: impl FnOnce(&Ruby) -> Result<Value, Error>,
) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let (html,) = a.required;
    let context = context_kwarg(ruby, Some(a.keywords));
    let document = document(ruby)?;
    unsafe {
        let doc = html_doc_unwrap(document)?;
        let (tag, ns) = resolve_fragment_context(doc, context)?;
        build_fragment_ctx(ruby, document, doc, html, tag, ns)
    }
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, self_: Value, rb_html: Value) -> Result<Value, Error> {
    let Some(context) = crate::glue::html_node::arg_node(&self_)?.element() else {
        return Err(Error::new(
            ruby.exception_arg_error(),
            "Node#parse requires an element context",
        ));
    };
    /* Only the context's tag and namespace ids are needed, read before the
     * fragment parse runs. */
    let (tag, ns) = (context.node().tag_id(), context.node().ns_id());
    unsafe {
        let document = keepalive_document(self_)?;
        let doc = html_doc_unwrap(document)?;
        let frag = build_fragment_ctx(ruby, document, doc, rb_html, tag, ns)?;
        frag.funcall("children", ())
    }
}

/// `Document#import_node(node, deep = false)` -> a copy of `node` owned by THIS
/// document - the DOM importNode, whose `deep` defaults to false.
///
/// Unlike `Node#clone_node` the copy belongs to the receiver, so this is the way
/// to bring a node across documents (Makiri never moves one between arenas). The
/// source is untouched and the copy is detached.
fn doc_import_node(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (Option<Value>,), (), (), (), ()>(args)?;
    let (node_v,) = a.required;
    let deep = a.optional.0.map(|v| v.to_bool()).unwrap_or(false);
    let _ = ruby;
    unsafe {
        let doc = html_doc_unwrap(self_)?;

        /* An XML node is TRANSLATED across representations (mkr -> lxb) into a
         * detached lxb subtree owned by this document. */
        if node_kind(node_v.as_raw()) == NODE_KIND_XML {
            let mut imp = core::ptr::null_mut();
            let xdoc =
                crate::glue::xml_node::doc_of(crate::glue::xml_node::xml_node_document(node_v)?);
            let src = crate::xml::model::NodeId::from_token(xml_node_unwrap(node_v)? as usize);
            xml_mut_check(cross_xml_to_html(
                doc.as_ptr() as *mut _,
                xdoc,
                src,
                deep,
                &mut imp,
            ))?;
            return Ok(Value::from_raw(wrap_html_node(
                RawNode::from_ptr(imp.cast()).expect("imported node"),
                self_.as_raw(),
            )));
        }

        let src = html_node_unwrap(node_v)?; /* Err on a non-node */
        let Some(imp) = import_with_fixup(doc, src, deep) else {
            return Err(Error::new(error_class(), "failed to import node"));
        };
        Ok(Value::from_raw(wrap_html_node(imp, self_.as_raw())))
    }
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
///
/// Built on the same import + `<template>`-content fixup as the fragment parser,
/// so a deep-cloned `<template>` carries its contents (which `import_node` alone
/// omits). Fails closed: a null import is an error rather than a partial node.
pub fn node_clone_node(rb_self: Value, args: &[Value]) -> Result<Value, Error> {
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
        return Err(Error::new(error_class(), "failed to clone node"));
    };
    let document = keepalive_document(rb_self)?;
    // SAFETY: `clone` is a detached node of `document`'s arena.
    Ok(unsafe { Value::from_raw(wrap_html_node(clone, document.as_raw())) })
}

/* ---- registration ---- */

/// `Init_makiri` calls this where it called the C one.
///
/// # Safety
/// Runs once, from `Init_makiri`, on the Ruby thread.
pub unsafe extern "C" fn init_document() {
    let ruby = Ruby::get().expect("init_document runs on the Ruby thread");
    let html_doc = magnus::RClass::from_value(CLASS_HTML_DOCUMENT.value())
        .expect("Makiri::HTML::Document is a class");

    html_doc
        .define_singleton_method("_parse", method!(doc_s_parse, 1))
        .expect("Document._parse");
    html_doc
        .define_method("root", method!(doc_root, 0))
        .expect("Document#root");
    html_doc
        .define_method("title", method!(doc_title, 0))
        .expect("Document#title");
    html_doc
        .define_method("errors", method!(doc_errors, 0))
        .expect("Document#errors");
    html_doc
        .define_method("internal_subset", method!(doc_internal_subset, 0))
        .expect("Document#internal_subset");
    html_doc
        .define_method("quirks_mode", method!(doc_quirks_mode, 0))
        .expect("Document#quirks_mode");
    html_doc
        .define_method("fragment", method!(doc_fragment, -1))
        .expect("Document#fragment");
    html_doc
        .define_method("import_node", method!(doc_import_node, -1))
        .expect("Document#import_node");

    let frag = magnus::RClass::from_value(CLASS_DOCUMENT_FRAGMENT.value())
        .expect("Makiri::DocumentFragment is a class");
    frag.define_singleton_method("parse", method!(frag_s_parse, -1))
        .expect("DocumentFragment.parse");

    /* Node#parse(html): fragment-parse in this element's context. Defined here,
     * next to the fragment machinery it reuses. */
    let node_methods = magnus::RModule::from_value(MOD_HTML_NODE_METHODS.value())
        .expect("Makiri::HTML::NodeMethods is a module");
    node_methods
        .define_method("parse", method!(node_parse, 1))
        .expect("Node#parse");

    let _ = ruby;
}
