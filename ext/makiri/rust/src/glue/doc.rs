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

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_char, c_int, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{method, prelude::*, Error, RString, Ruby, Value};
use rb_sys::{rb_data_type_t, VALUE};

use crate::lexbor_abi as lxb;

use super::abi::{
    cXmlDocument, error_class, html_node_unwrap, keepalive_document, mkr_cDocumentFragment,
    mkr_cHtmlDocument, mkr_mHtmlNodeMethods, ruby_copy_bytes, ruby_str_known_valid_utf8,
    ruby_to_utf8, wrap_html_node, xml_node_unwrap, DataType, LxbDoc, LxbNode,
    LXB_DOM_NODE_TYPE_ELEMENT,
};
use super::fragment::{
    build_fragment_ctx, context_kwarg, import_with_fixup, resolve_fragment_context,
};

/* ------------------------------------------------------------------ *
 * the wrapper                                                        *
 * ------------------------------------------------------------------ */

/// `mkr_doc_data_t`: the parsed handle (owned - GC frees it) and the reserved
/// errors Array.
struct DocData {
    parsed: *mut crate::dom_adapter::post_parse::Parsed,
    errors: VALUE,
}

/// Generated, not transcribed. A hand-written 1 here (it is 2) made
/// `import_node` treat every HTML node as an XML one.
const NODE_KIND_XML: c_int = lxb::mkr::mkr_node_kind_t_MKR_NODE_KIND_XML as c_int;

pub use crate::dom_adapter::cross_import::cross_xml_to_html;
pub use crate::dom_adapter::post_parse::parse_html;
pub use crate::dom_adapter::post_parse::parsed_destroy;
pub use crate::dom_adapter::post_parse::parsed_html_doc;
pub use crate::dom_adapter::post_parse::parsed_kind;
pub use crate::glue::node::node_kind;
pub use crate::glue::xml_node::mutate::xml_mut_check;
pub use crate::xml::api::xml_doc_memsize;

extern "C" {

    fn lxb_dom_document_root(doc: *mut LxbDoc) -> *mut LxbNode;
    fn lxb_html_document_title(doc: *mut c_void, len: *mut usize) -> *const u8;
}

/// The doctype node type, generated (see lexbor_abi).
const NODE_TYPE_DOCUMENT_TYPE: u32 = super::abi::LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;

/// Stated once, in `lexbor_abi::mkr`, rather than restated here.
const DOC_XML: u32 = lxb::mkr::mkr_doc_kind_t_MKR_DOC_XML;

unsafe extern "C" fn doc_mark(ptr: *mut c_void) {
    let d = &*(ptr as *const DocData);
    rb_sys::rb_gc_mark(d.errors);
}

unsafe extern "C" fn doc_free(ptr: *mut c_void) {
    let d = ptr as *mut DocData;
    if !(*d).parsed.is_null() {
        parsed_destroy((*d).parsed);
    }
    rb_sys::ruby_xfree(ptr);
}

unsafe extern "C" fn doc_memsize(ptr: *const c_void) -> rb_sys::size_t {
    let d = &*(ptr as *const DocData);
    let mut total = core::mem::size_of::<DocData>();
    // Lexbor's arena size is not cheaply queryable, so an HTML document reports
    // the wrapper only; the XML arena tracks its own byte total.
    if !d.parsed.is_null() && parsed_kind(d.parsed) == DOC_XML {
        total += xml_doc_memsize(&*(super::abi::parsed_xml_doc(d.parsed) as *const _));
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
#[allow(non_upper_case_globals)]
pub static doc_type: DataType = doc_data_type(c"Makiri::Document".as_ptr(), core::ptr::null());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `html_doc_unwrap` - which
/// reinterprets the handle as a Lexbor document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
static HTML_DOC_TYPE: DataType =
    doc_data_type(c"Makiri::HTML::Document".as_ptr(), doc_type.as_ptr());
static XML_DOC_TYPE: DataType = doc_data_type(c"Makiri::XML::Document".as_ptr(), doc_type.as_ptr());

/// The Lexbor document behind an HTML Document. `Err(TypeError)` otherwise.
pub unsafe fn html_doc_unwrap(rb_doc: VALUE) -> Result<*mut lxb::lxb_dom_document_t, Error> {
    let d = crate::bridge::ruby::typed_data(rb_doc, HTML_DOC_TYPE.as_ptr())? as *mut DocData;
    Ok(html_doc_of(d))
}

/// [`html_doc_unwrap`] for a VALUE already known to be an HTML Document.
pub unsafe fn html_doc_known(rb_doc: VALUE) -> *mut lxb::lxb_dom_document_t {
    html_doc_of(
        crate::bridge::ruby::typed_data_known(rb_doc, HTML_DOC_TYPE.as_ptr()) as *mut DocData,
    )
}

unsafe fn html_doc_of(d: *mut DocData) -> *mut lxb::lxb_dom_document_t {
    /* An lxb_html_document_t leads with its lxb_dom_document_t, so this is a
     * downcast to the embedded base, not a reinterpretation. */
    parsed_html_doc((*d).parsed) as *mut lxb::lxb_dom_document_t
}

/// The parsed handle behind any Document. `Err(TypeError)` for a non-Document.
pub unsafe fn doc_parsed(
    rb_doc: VALUE,
) -> Result<*mut crate::dom_adapter::post_parse::Parsed, Error> {
    let d = crate::bridge::ruby::typed_data(rb_doc, doc_type.as_ptr())? as *mut DocData;
    Ok((*d).parsed)
}

/// [`doc_parsed`] for a VALUE already known to be a Document - a node's
/// keepalive Document, or the receiver of a Document method.
pub unsafe fn doc_parsed_known(rb_doc: VALUE) -> *mut crate::dom_adapter::post_parse::Parsed {
    (*(crate::bridge::ruby::typed_data_known(rb_doc, doc_type.as_ptr()) as *mut DocData)).parsed
}

/// Wrap an owned handle as a Document; GC takes ownership. The leaf class is
/// chosen by kind - a Lexbor-backed handle is an HTML Document, an arena-backed
/// one an XML Document.
pub unsafe extern "C" fn wrap_document(
    parsed: *mut crate::dom_adapter::post_parse::Parsed,
) -> VALUE {
    let is_xml = parsed_kind(parsed) == DOC_XML;
    let (klass, ty) = if is_xml {
        (cXmlDocument, XML_DOC_TYPE.as_ptr())
    } else {
        (mkr_cHtmlDocument, HTML_DOC_TYPE.as_ptr())
    };
    /* The errors array is created AFTER the wrap. Created before, it would sit
     * in this malloc'd struct - seen by no mark - across the wrap's allocation,
     * and a GC there frees it; `doc_mark` then marks a dead slot ("try to mark
     * T_NONE object" under GC_COMPACT_STRESS). */
    crate::bridge::ruby::wrap_zeroed::<DocData>(
        klass,
        ty,
        |d| d.parsed = parsed,
        |d| d.errors = rb_sys::rb_ary_new(),
    )
}

/* ---- Document.parse ---- */

/// Arguments for the GVL-released parse.
struct ParseArgs {
    src: *const u8,
    len: usize,
    assume_valid: bool,
    result: *mut crate::dom_adapter::post_parse::Parsed,
}

/// Runs with the GVL released: pure C (Lexbor + libc), touching no Ruby state.
unsafe extern "C" fn parse_nogvl(p: *mut c_void) -> *mut c_void {
    let a = &mut *(p as *mut ParseArgs);
    a.result = parse_html(a.src, a.len, a.assume_valid);
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
        let mut owned = match ruby_copy_bytes(src) {
            Some(owned) => owned,
            None => return Err(Error::new(error_class(), "out of memory copying source")),
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
                data.errors = rb_sys::rb_ary_new();
                d = data;
            },
        );

        let mut args = ParseArgs {
            src: owned.ptr as *const u8,
            len: owned.len,
            assume_valid,
            result: core::ptr::null_mut(),
        };
        rb_sys::rb_thread_call_without_gvl(
            Some(parse_nogvl),
            &mut args as *mut ParseArgs as *mut c_void,
            None,
            core::ptr::null_mut(),
        );
        owned.clear();

        (*d).parsed = args.result;
        if (*d).parsed.is_null() {
            return Err(Error::new(error_class(), "failed to parse HTML document"));
        }
        let _ = ruby;
        Ok(Value::from_raw(obj))
    }
}

/* ---- read-only accessors ---- */

fn doc_root(ruby: &Ruby, self_: Value) -> Value {
    let _ = ruby;
    unsafe {
        let doc = html_doc_known(self_.as_raw());
        Value::from_raw(wrap_html_node(lxb_dom_document_root(doc), self_.as_raw()))
    }
}

/// The document `<title>`, or `""`.
fn doc_title(ruby: &Ruby, self_: Value) -> RString {
    unsafe {
        let mut len: usize = 0;
        let doc = html_doc_known(self_.as_raw());
        let s = lxb_html_document_title(doc as *mut c_void, &mut len);
        let bytes: &[u8] = if s.is_null() {
            &[]
        } else {
            core::slice::from_raw_parts(s, len)
        };
        ruby.enc_str_new(bytes, ruby.utf8_encoding())
    }
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(ruby: &Ruby, self_: Value) -> Value {
    unsafe {
        let doc = html_doc_known(self_.as_raw()) as *mut LxbNode;
        let mut c = (*doc).first_child;
        while !c.is_null() {
            if (*c).type_ == NODE_TYPE_DOCUMENT_TYPE {
                return Value::from_raw(wrap_html_node(c, self_.as_raw()));
            }
            c = (*c).next;
        }
        ruby.qnil().as_value()
    }
}

/// The quirks mode as an Integer matching Lexbor (and Gumbo/Nokogiri):
/// 0 no-quirks, 1 quirks, 2 limited-quirks. Set by the parser from the doctype.
fn doc_quirks_mode(ruby: &Ruby, self_: Value) -> Value {
    let _ = ruby;
    unsafe {
        let doc = html_doc_known(self_.as_raw());
        Value::from_raw(rb_sys::rb_int2inum((*doc).compat_mode as isize))
    }
}

/// Parse warnings. Reserved; currently always empty.
fn doc_errors(ruby: &Ruby, self_: Value) -> Value {
    let _ = ruby;
    unsafe {
        /* A Document method, so the receiver is a Document. */
        let d = crate::bridge::ruby::typed_data_known(self_.as_raw(), doc_type.as_ptr())
            as *mut DocData;
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
        let parsed = parse_html(SHELL.as_ptr(), SHELL.len(), true);
        if parsed.is_null() {
            return Err(Error::new(
                error_class(),
                "failed to create fragment document",
            ));
        }
        Ok(Value::from_raw(wrap_document(parsed))) /* GC owns parsed now */
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
        let doc = html_doc_unwrap(document.as_raw())?;
        let (tag, ns) = resolve_fragment_context(doc, context)?;
        build_fragment_ctx(ruby, document, doc, html, tag, ns)
    }
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, self_: Value, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = html_node_unwrap(self_.as_raw())?;
        if (*node).type_ != LXB_DOM_NODE_TYPE_ELEMENT {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "Node#parse requires an element context",
            ));
        }
        let document = Value::from_raw(keepalive_document(self_.as_raw())?);
        let doc = html_doc_unwrap(document.as_raw())?;
        let frag =
            build_fragment_ctx(ruby, document, doc, rb_html, (*node).local_name, (*node).ns)?;
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
        let doc = html_doc_unwrap(self_.as_raw())?;

        /* An XML node is TRANSLATED across representations (mkr -> lxb) into a
         * detached lxb subtree owned by this document. */
        if node_kind(node_v.as_raw()) == NODE_KIND_XML {
            let mut imp: *mut LxbNode = core::ptr::null_mut();
            let xdoc = crate::glue::xml_node::doc_of(crate::glue::xml_node::xml_node_document(
                node_v.as_raw(),
            )?);
            let src =
                crate::xml::model::NodeId::from_token(xml_node_unwrap(node_v.as_raw())? as usize);
            xml_mut_check(cross_xml_to_html(doc, xdoc, src, deep, &mut imp));
            return Ok(Value::from_raw(wrap_html_node(imp, self_.as_raw())));
        }

        let src = html_node_unwrap(node_v.as_raw())?; /* Err on a non-node */
        let Some(imp) = import_with_fixup(doc, src, deep) else {
            return Err(Error::new(error_class(), "failed to import node"));
        };
        Ok(Value::from_raw(wrap_html_node(imp, self_.as_raw())))
    }
}

/// `Node#clone_node(deep = false)`: a copy owned by the same document and
/// detached from any parent - the DOM cloneNode, whose `deep` defaults to false.
///
/// Exported with the C method signature because `ruby_html_node.c` registers it.
/// Built on the same import + `<template>`-content fixup as the fragment parser,
/// so a deep-cloned `<template>` carries its contents (which `import_node` alone
/// omits). Fails closed: a null import raises rather than returning a partial
/// node.
pub unsafe extern "C" fn node_clone_node(argc: c_int, argv: *const VALUE, self_: VALUE) -> VALUE {
    let mut deep_v: VALUE = rb_sys::Qnil as VALUE;
    rb_sys::rb_scan_args(argc, argv, c"01".as_ptr(), &mut deep_v);
    /* RTEST: anything but nil and false. */
    let deep = deep_v != rb_sys::Qnil as VALUE && deep_v != rb_sys::Qfalse as VALUE;

    /* Ruby calls this with the C convention, so a failure is raised here,
     * before anything is owned. */
    let node = match html_node_unwrap(self_) {
        Ok(node) => node,
        Err(e) => crate::bridge::ruby::raise(e),
    };
    let doc = (*node).owner_document;

    let Some(clone) = import_with_fixup(doc, node, deep) else {
        super::abi::rb_raise(super::abi::mkr_eError, c"failed to clone node".as_ptr());
    };
    let document = match keepalive_document(self_) {
        Ok(document) => document,
        Err(e) => crate::bridge::ruby::raise(e),
    };
    wrap_html_node(clone, document)
}

/* ---- registration ---- */

/// `Init_makiri` calls this where it called the C one.
///
/// # Safety
/// Runs once, from `Init_makiri`, on the Ruby thread.
pub unsafe extern "C" fn init_document() {
    let ruby = Ruby::get().expect("init_document runs on the Ruby thread");
    let html_doc = magnus::RClass::from_value(Value::from_raw(mkr_cHtmlDocument))
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

    let frag = magnus::RClass::from_value(Value::from_raw(mkr_cDocumentFragment))
        .expect("Makiri::DocumentFragment is a class");
    frag.define_singleton_method("parse", method!(frag_s_parse, -1))
        .expect("DocumentFragment.parse");

    /* Node#parse(html): fragment-parse in this element's context. Defined here,
     * next to the fragment machinery it reuses. */
    let node_methods = magnus::RModule::from_value(Value::from_raw(mkr_mHtmlNodeMethods))
        .expect("Makiri::HTML::NodeMethods is a module");
    node_methods
        .define_method("parse", method!(node_parse, 1))
        .expect("Node#parse");

    let _ = ruby;
}
