//! `Makiri::HTML::Document` and the fragment machinery (glue/ruby_doc.c).
//!
//!   `Document._parse(source)`, `#root`, `#title`, `#errors`,
//!   `#internal_subset`, `#quirks_mode`, `#fragment(html, context:)`,
//!   `#import_node(node, deep = false)`
//!   `DocumentFragment.parse(html, context:)`, `Node#parse(html)`
//!
//! # This file is mostly a C ABI, not a Ruby one
//!
//! Ten methods, and eleven symbols other translation units call. The Document
//! wrapper type and its `rb_data_type_t` chain, the parsed-handle accessors, and
//! every piece of the fragment pipeline are consumed by `ruby_html_node.c`,
//! `ruby_html_mutate.c` and `cross_import.c`, which are still C - so the port
//! keeps each signature exactly, and `glue::abi` grew a compile-time check that
//! the definitions here match the declarations there (rustc does not check a
//! `#[no_mangle]` definition against an `extern` block, which is how one symbol
//! could otherwise get two types again).
//!
//! # Parsing releases the GVL
//!
//! `mkr_parse_html` and everything under it is Ruby-free, and a freshly parsed
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
    error_class, LxbNode, is_kind_of, mkr_cDocumentFragment, mkr_cHtmlDocument, mkr_cNode,
    mkr_cXmlDocument, mkr_html_node_unwrap, mkr_mHtmlNodeMethods, mkr_node_document,
    mkr_wrap_html_node, DataType,
};

/* ------------------------------------------------------------------ *
 * the wrapper                                                        *
 * ------------------------------------------------------------------ */

/// `mkr_doc_data_t`: the parsed handle (owned - GC frees it) and the reserved
/// errors Array.
#[repr(C)]
struct DocData {
    parsed: *mut c_void,
    errors: VALUE,
}

extern "C" {
    fn mkr_parsed_kind(p: *const c_void) -> c_int;
    fn mkr_parsed_html_doc(p: *const c_void) -> *mut lxb::lxb_dom_document_t;
    fn mkr_parsed_destroy(p: *mut c_void);
    fn mkr_parse_html(src: *const u8, len: usize, assume_valid: bool) -> *mut c_void;
    fn mkr_xml_doc_memsize(xdoc: *const c_void) -> usize;
}

/// Generated, not transcribed (lexbor_abi::mkr).
const MKR_DOC_XML: c_int = lxb::mkr::mkr_doc_kind_t_MKR_DOC_XML as c_int;

unsafe extern "C" fn doc_mark(ptr: *mut c_void) {
    let d = &*(ptr as *const DocData);
    rb_sys::rb_gc_mark(d.errors);
}

unsafe extern "C" fn doc_free(ptr: *mut c_void) {
    let d = ptr as *mut DocData;
    if !(*d).parsed.is_null() {
        mkr_parsed_destroy((*d).parsed);
    }
    rb_sys::ruby_xfree(ptr);
}

unsafe extern "C" fn doc_memsize(ptr: *const c_void) -> rb_sys::size_t {
    let d = &*(ptr as *const DocData);
    let mut total = core::mem::size_of::<DocData>();
    // Lexbor's arena size is not cheaply queryable, so an HTML document reports
    // the wrapper only; the XML arena tracks its own byte total.
    if !d.parsed.is_null() && mkr_parsed_kind(d.parsed) == MKR_DOC_XML {
        total += mkr_xml_doc_memsize(super::abi::mkr_parsed_xml_doc(d.parsed));
    }
    total as rb_sys::size_t
}

const fn doc_data_type(name: *const c_char, parent: *const rb_data_type_t) -> DataType {
    DataType::new(name, parent, Some(doc_mark), Some(doc_free), Some(doc_memsize))
}

/// The base type, exported: the kind-agnostic accessors (`mkr_doc_parsed`,
/// `#errors`) legitimately accept either representation.
#[no_mangle]
pub static mkr_doc_type: DataType = doc_data_type(c"Makiri::Document".as_ptr(), core::ptr::null());

/// HTML and XML Documents share the layout and the GC functions but are wrapped
/// under DISTINCT types deriving from the base, so `mkr_html_doc_unwrap` - which
/// reinterprets the handle as a Lexbor document - raises TypeError on an XML
/// Document through Ruby's own type machinery rather than relying on an assert
/// that NDEBUG erases.
static MKR_HTML_DOC_TYPE: DataType =
    doc_data_type(c"Makiri::HTML::Document".as_ptr(), mkr_doc_type.as_ptr());
static MKR_XML_DOC_TYPE: DataType =
    doc_data_type(c"Makiri::XML::Document".as_ptr(), mkr_doc_type.as_ptr());

/// The Lexbor document behind an HTML Document. **Raises** TypeError otherwise.
#[no_mangle]
pub unsafe extern "C" fn mkr_html_doc_unwrap(rb_doc: VALUE) -> *mut lxb::lxb_dom_document_t {
    let d = rb_sys::rb_check_typeddata(rb_doc, MKR_HTML_DOC_TYPE.as_ptr()) as *mut DocData;
    mkr_parsed_html_doc((*d).parsed)
}

/// The parsed handle behind any Document. **Raises** TypeError for a non-Document.
#[no_mangle]
pub unsafe extern "C" fn mkr_doc_parsed(rb_doc: VALUE) -> *mut c_void {
    let d = rb_sys::rb_check_typeddata(rb_doc, mkr_doc_type.as_ptr()) as *mut DocData;
    (*d).parsed
}

/// Wrap an owned handle as a Document; GC takes ownership. The leaf class is
/// chosen by kind - a Lexbor-backed handle is an HTML Document, an arena-backed
/// one an XML Document.
#[no_mangle]
pub unsafe extern "C" fn mkr_wrap_document(parsed: *mut c_void) -> VALUE {
    let is_xml = mkr_parsed_kind(parsed) == MKR_DOC_XML;
    let (klass, ty) = if is_xml {
        (mkr_cXmlDocument, MKR_XML_DOC_TYPE.as_ptr())
    } else {
        (mkr_cHtmlDocument, MKR_HTML_DOC_TYPE.as_ptr())
    };
    let d = rb_sys::ruby_xcalloc(1, core::mem::size_of::<DocData>() as rb_sys::size_t)
        as *mut DocData;
    (*d).parsed = parsed;
    (*d).errors = rb_sys::rb_ary_new();
    rb_sys::rb_data_typed_object_wrap(klass, d as *mut c_void, ty)
}

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

type LxbDoc = lxb::lxb_dom_document_t;

/// `mkr_owned_bytes_t`.
#[repr(C)]
struct OwnedBytes {
    ptr: *mut c_char,
    len: usize,
}

/// `mkr_ruby_borrowed_bytes_t`.
#[repr(C)]
#[derive(Clone, Copy)]
struct BorrowedBytes {
    value: VALUE,
    ptr: *const c_char,
    len: usize,
}

/// `mkr_ruby_borrowed_text_t`.
#[repr(C)]
#[derive(Clone, Copy)]
struct BorrowedText {
    value: VALUE,
    ptr: *const c_char,
    len: usize,
}

extern "C" {
    fn mkr_ruby_to_utf8(v: VALUE) -> VALUE;
    fn mkr_ruby_bytes_view(v: VALUE) -> BorrowedBytes;
    fn mkr_ruby_copy_bytes(v: VALUE, out: *mut OwnedBytes) -> c_int;
    fn mkr_ruby_str_known_valid_utf8(v: VALUE) -> bool;
    fn mkr_ruby_verified_text(v: VALUE, what: *const c_char) -> BorrowedText;
    fn mkr_utf8_sanitize(src: *const u8, len: usize, out: *mut *mut u8, out_len: *mut usize)
        -> c_int;
    fn mkr_reallocarray(p: *mut c_void, count: usize, elem: usize) -> *mut c_void;

    fn mkr_node_kind(v: VALUE) -> c_int;
    fn mkr_xml_node_unwrap(v: VALUE) -> *mut c_void;
    fn mkr_cross_xml_to_html(
        doc: *mut LxbDoc,
        src: *mut c_void,
        deep: bool,
        out: *mut *mut LxbNode,
    ) -> c_int;
    fn mkr_xml_mut_check(status: c_int);

    fn lxb_html_parser_create() -> *mut c_void;
    fn lxb_html_parser_init(parser: *mut c_void) -> u32;
    fn lxb_html_parser_destroy(parser: *mut c_void) -> *mut c_void;
    fn lxb_html_parse_fragment_by_tag_id(
        parser: *mut c_void,
        doc: *mut c_void,
        tag: usize,
        ns: usize,
        src: *const u8,
        len: usize,
    ) -> *mut LxbNode;
    fn lxb_dom_document_fragment_interface_create(doc: *mut LxbDoc) -> *mut c_void;
    fn lxb_dom_document_import_node(doc: *mut LxbDoc, node: *mut LxbNode, deep: bool)
        -> *mut LxbNode;
    fn lxb_dom_node_insert_child(to: *mut LxbNode, node: *mut LxbNode);
    fn lxb_dom_node_insert_before(to: *mut LxbNode, node: *mut LxbNode);
    fn lxb_dom_document_root(doc: *mut LxbDoc) -> *mut LxbNode;
    fn lxb_html_document_title(doc: *mut c_void, len: *mut usize) -> *const u8;
    /// The `_noi` twin: Lexbor publishes the plain name as `lxb_inline`, which
    /// has no symbol to link against.
    #[link_name = "lxb_tag_id_by_name_noi"]
    fn lxb_tag_id_by_name(hash: *mut c_void, name: *const u8, len: usize) -> usize;
}

/// The shared pre-order walk (`mkr_dom_preorder_next`), which is `static
/// inline` in C and therefore has no symbol to call.
///
/// Restating it rather than exporting the C one keeps the DoS-avoiding
/// invariant it carries - a parent-pointer walk, never recursion - visible at
/// the one place this file relies on it.
#[inline]
unsafe fn preorder_next(mut node: *mut LxbNode, root: *mut LxbNode) -> *mut LxbNode {
    if !(*node).first_child.is_null() {
        return (*node).first_child;
    }
    while node != root && (*node).next.is_null() {
        node = (*node).parent;
    }
    if node == root {
        return core::ptr::null_mut();
    }
    (*node).next
}

/// `mkr_owned_bytes_clear`, also `static inline` in C.
#[inline]
unsafe fn owned_bytes_clear(o: &mut OwnedBytes) {
    if !o.ptr.is_null() {
        libc_free(o.ptr as *mut c_void);
    }
    o.ptr = core::ptr::null_mut();
    o.len = 0;
}

/// Generated, not transcribed. A hand-written 1 here (it is 2) made
/// `import_node` treat every HTML node as an XML one.
const MKR_NODE_KIND_XML: c_int = lxb::mkr::mkr_node_kind_t_MKR_NODE_KIND_XML as c_int;

/// Lexbor node types and the tag/namespace ids this file compares against.
/// Generated, so a pin that renumbers them is a build-time change, not a
/// silently different answer (see lexbor_abi).
const NODE_TYPE_ELEMENT: u32 = super::abi::LXB_DOM_NODE_TYPE_ELEMENT;
const NODE_TYPE_DOCUMENT_TYPE: u32 = super::abi::LXB_DOM_NODE_TYPE_DOCUMENT_TYPE;
const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;
const NS_SVG: usize = lxb::lxb_ns_id_enum_t_LXB_NS_SVG as usize;
const NS_MATH: usize = lxb::lxb_ns_id_enum_t_LXB_NS_MATH as usize;

/// The content fragment of a `<template>`, as a node, or null.
unsafe fn template_content(n: *mut LxbNode) -> *mut LxbNode {
    let t = n as *mut lxb::lxb_html_template_element_t;
    let frag = (*t).content;
    if frag.is_null() {
        core::ptr::null_mut()
    } else {
        frag as *mut LxbNode /* a fragment begins with its node */
    }
}

unsafe fn is_html_template(n: *const LxbNode) -> bool {
    (*n).type_ == NODE_TYPE_ELEMENT
        && (*n).local_name == lxb::lxb_tag_id_enum_t_LXB_TAG_TEMPLATE as usize
        && (*n).ns == NS_HTML
}

/// `lxb_dom_document_import_node` deep-clones the normal child chain but NOT a
/// `<template>`'s separate content fragment, so an imported template comes out
/// empty. Walk source and clone in lockstep (deep import preserves child order
/// 1:1) and import each template's content into the clone's.
///
/// Iterative, with an explicit worklist: an adversarially deep fragment must not
/// be able to overflow the stack. Best-effort on allocation failure, as the C
/// was - a template whose content could not be copied is left empty rather than
/// the whole import failing.
unsafe fn fixup_template_content(doc: *mut LxbDoc, root_src: *mut LxbNode, root_clone: *mut LxbNode) {
    let mut stack: Vec<(*mut LxbNode, *mut LxbNode)> = Vec::new();
    if !crate::falloc::try_push(&mut stack, (root_src, root_clone)) {
        return;
    }

    while let Some((src_root, clone_root)) = stack.pop() {
        let mut sn = src_root;
        let mut cn = clone_root;
        while !sn.is_null() && !cn.is_null() {
            if is_html_template(sn) && is_html_template(cn) {
                // Lexbor's `lxb_html_interface_template` is a cast macro, so
                // reaching the content fragment is a cast plus a field read.
                let sc = template_content(sn);
                let cc = template_content(cn);
                if !sc.is_null() && !cc.is_null() {
                    let mut x = (*sc).first_child;
                    while !x.is_null() {
                        let imp = lxb_dom_document_import_node(doc, x, true);
                        if !imp.is_null() {
                            lxb_dom_node_insert_child(cc, imp);
                        }
                        x = (*x).next;
                    }
                    if !crate::falloc::try_push(&mut stack, (sc, cc)) {
                        return;
                    }
                }
            }
            sn = preorder_next(sn, src_root);
            cn = preorder_next(cn, clone_root);
        }
    }
}

/// Browser-compatible decoding for fragment input: invalid UTF-8 becomes
/// U+FFFD, valid input is used in place. `-1` on OOM with nothing allocated, so
/// the caller can release its parser before raising.
#[no_mangle]
pub unsafe extern "C" fn mkr_sanitize_html_input(
    html: VALUE,
    out: *mut *const u8,
    out_len: *mut usize,
    owned: *mut *mut u8,
) -> c_int {
    let u8v = mkr_ruby_to_utf8(html);
    let hv = mkr_ruby_bytes_view(u8v);

    if u8v != html {
        // Transcoded: a fresh String nothing keeps alive past this return, so
        // its bytes must NOT be borrowed. It is already valid UTF-8, so copy
        // rather than sanitise.
        let n = if hv.len > 0 { hv.len } else { 1 };
        let buf = mkr_reallocarray(core::ptr::null_mut(), n, 1) as *mut u8;
        if buf.is_null() {
            return -1;
        }
        if hv.len > 0 {
            core::ptr::copy_nonoverlapping(hv.ptr as *const u8, buf, hv.len);
        }
        *owned = buf;
        *out = buf;
        *out_len = hv.len;
        return 0;
    }

    // Not transcoded: input Ruby already knows is valid UTF-8 is borrowed in
    // place (the caller keeps `html` alive); anything else is sanitised.
    if mkr_ruby_str_known_valid_utf8(html) {
        *owned = core::ptr::null_mut();
        *out = hv.ptr as *const u8;
        *out_len = hv.len;
        return 0;
    }
    let mut clean: *mut u8 = core::ptr::null_mut();
    let mut clean_len: usize = 0;
    if mkr_utf8_sanitize(hv.ptr as *const u8, hv.len, &mut clean, &mut clean_len) != 0 {
        return -1;
    }
    *owned = clean;
    *out = if clean.is_null() { hv.ptr as *const u8 } else { clean };
    *out_len = if clean.is_null() { hv.len } else { clean_len };
    0
}

#[no_mangle]
pub unsafe extern "C" fn mkr_emit_append(imported: *mut LxbNode, u: *mut c_void) {
    lxb_dom_node_insert_child(u as *mut LxbNode, imported);
}

#[no_mangle]
pub unsafe extern "C" fn mkr_emit_before(imported: *mut LxbNode, u: *mut c_void) {
    lxb_dom_node_insert_before(u as *mut LxbNode, imported);
}

#[no_mangle]
pub unsafe extern "C" fn mkr_import_fragment_children(
    doc: *mut LxbDoc,
    root: *mut LxbNode,
    emit: unsafe extern "C" fn(*mut LxbNode, *mut c_void),
    u: *mut c_void,
) {
    let mut f = (*root).first_child;
    while !f.is_null() {
        let next = (*f).next; /* import does not unlink f, but be safe */
        let imp = lxb_dom_document_import_node(doc, f, true);
        if !imp.is_null() {
            emit(imp, u);
            fixup_template_content(doc, f, imp);
        }
        f = next;
    }
}

/// `mkr_fragment_parse_fn`.
type FragmentParseFn =
    unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut c_void) -> *mut LxbNode;

/// Run a fragment parse with a fresh parser. **Raises** on failure.
///
/// The parser is destroyed before any raise: the fragment tree belongs to its
/// document, not the parser, so it survives - the caller may still read
/// `root->owner_document` afterwards.
#[no_mangle]
pub unsafe extern "C" fn mkr_run_fragment_parser(
    html: VALUE,
    parse: FragmentParseFn,
    ctx: *mut c_void,
) -> *mut LxbNode {
    let parser = lxb_html_parser_create();
    if parser.is_null() || lxb_html_parser_init(parser) != 0 {
        if !parser.is_null() {
            lxb_html_parser_destroy(parser);
        }
        super::abi::rb_raise(super::abi::mkr_eError, c"failed to create HTML parser".as_ptr());
    }

    let mut hsrc: *const u8 = core::ptr::null();
    let mut hlen: usize = 0;
    let mut owned: *mut u8 = core::ptr::null_mut();
    if mkr_sanitize_html_input(html, &mut hsrc, &mut hlen, &mut owned) != 0 {
        lxb_html_parser_destroy(parser);
        super::abi::rb_raise(
            super::abi::mkr_eError,
            c"out of memory decoding fragment HTML".as_ptr(),
        );
    }

    let root = parse(parser, hsrc, hlen, ctx);
    if !owned.is_null() {
        libc_free(owned as *mut c_void); /* consumed by the parse; free on every path */
    }
    lxb_html_parser_destroy(parser);
    if root.is_null() {
        super::abi::rb_raise(super::abi::mkr_eError, c"failed to parse HTML fragment".as_ptr());
    }
    root
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

/// Deep-import `src` into `doc`, `<template>` contents included. **Raises**
/// rather than returning a partial node.
#[no_mangle]
pub unsafe extern "C" fn mkr_html_import_deep(
    doc: *mut LxbDoc,
    src: *mut LxbNode,
) -> *mut LxbNode {
    let imp = lxb_dom_document_import_node(doc, src, true);
    if imp.is_null() {
        super::abi::rb_raise(super::abi::mkr_eError, c"failed to import node".as_ptr());
    }
    fixup_template_content(doc, src, imp);
    imp
}

/* ------------------------------------------------------------------ *
 * fragment context + the Ruby surface                                *
 * ------------------------------------------------------------------ */

/// Resolve a fragment-parsing context - the element the HTML is parsed "inside
/// of", per the WHATWG algorithm - into a tag id and namespace.
///
/// Matches Nokogiri's `context:`: nil is `<body>` in the HTML namespace; a node
/// contributes its own tag and namespace (the only way to reach a foreign
/// non-root context such as SVG `<desc>`); a String names an HTML-namespace tag,
/// except "svg" / "math" which name the foreign roots.
///
/// **Raises** on an unusable context.
unsafe fn resolve_fragment_context(doc: *mut LxbDoc, context: Option<Value>) -> (usize, usize) {
    let Some(context) = context else {
        return (lxb::lxb_tag_id_enum_t_LXB_TAG_BODY as usize, NS_HTML);
    };
    if context.is_nil() {
        return (lxb::lxb_tag_id_enum_t_LXB_TAG_BODY as usize, NS_HTML);
    }

    if is_kind_of(context, mkr_cNode) {
        /* Reject an XML node before any Lexbor use. */
        let cn = mkr_html_node_unwrap(context.as_raw());
        if (*cn).type_ != NODE_TYPE_ELEMENT {
            rb_sys::rb_raise(
                rb_sys::rb_eArgError,
                c"fragment context node must be an element".as_ptr(),
            );
        }
        return ((*cn).local_name, (*cn).ns);
    }

    /* A context tag name is a programmatic control string, not parsed HTML, so
     * it follows the strict text-input contract (valid UTF-8, no NUL). */
    let cv = mkr_ruby_verified_text(context.as_raw(), c"fragment context element".as_ptr());
    let name = if cv.ptr.is_null() || cv.len == 0 {
        &[][..]
    } else {
        core::slice::from_raw_parts(cv.ptr as *const u8, cv.len)
    };
    if name == b"svg" {
        return (lxb::lxb_tag_id_enum_t_LXB_TAG_SVG as usize, NS_SVG);
    }
    if name == b"math" {
        return (lxb::lxb_tag_id_enum_t_LXB_TAG_MATH as usize, NS_MATH);
    }
    let tid = lxb_tag_id_by_name((*doc).tags as *mut c_void, name.as_ptr(), name.len());
    if tid == lxb::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize {
        // The C wrote `"...: %" PRIsVALUE` - two string literals the C
        // preprocessor joins. Rust has no such concatenation, so carrying the
        // line over verbatim produced the literal `%" PRIsVALUE` in the
        // message; the differential caught it. `%.*s` over the verified bytes
        // prints what PRIsVALUE printed for a String: its content.
        rb_sys::rb_raise(
            rb_sys::rb_eArgError,
            c"unknown fragment context element: %.*s".as_ptr(),
            name.len() as c_int,
            name.as_ptr(),
        );
    }
    (tid, NS_HTML)
}

/// Parse callback for `mkr_run_fragment_parser`: Lexbor's by-tag-id parser,
/// which implements the full algorithm for the context (tokenizer state for
/// rawtext/rcdata, foreign-content adjustment, the form pointer).
#[repr(C)]
struct FragTagCtx {
    doc: *mut LxbDoc,
    tag: usize,
    ns: usize,
}

unsafe extern "C" fn parse_fragment_by_tag(
    parser: *mut c_void,
    hsrc: *const u8,
    hlen: usize,
    ctx: *mut c_void,
) -> *mut LxbNode {
    let c = &*(ctx as *const FragTagCtx);
    lxb_html_parse_fragment_by_tag_id(parser, c.doc as *mut c_void, c.tag, c.ns, hsrc, hlen)
}

/// Parse `html` in the given context and build a DOCUMENT_FRAGMENT owned by
/// `document`, so its nodes can be spliced into it.
unsafe fn build_fragment_ctx(
    ruby: &Ruby,
    document: Value,
    rb_html: Value,
    tag: usize,
    ns: usize,
) -> Result<Value, Error> {
    let html = ruby.into_value(rb_html.to_r_string()?);
    let doc = mkr_html_doc_unwrap(document.as_raw());

    let frag = lxb_dom_document_fragment_interface_create(doc);
    if frag.is_null() {
        return Err(Error::new(error_class(), "failed to create document fragment"));
    }
    let frag_node = frag as *mut LxbNode;

    let pctx = FragTagCtx { doc, tag, ns };
    let root = mkr_run_fragment_parser(
        html.as_raw(),
        parse_fragment_by_tag,
        &pctx as *const FragTagCtx as *mut c_void,
    );
    mkr_import_fragment_children(doc, root, mkr_emit_append, frag_node as *mut c_void);
    let out = mkr_wrap_html_node(frag_node, document.as_raw());
    Ok(Value::from_raw(out))
}

/// The `context:` keyword, or None.
fn context_kwarg(ruby: &Ruby, kw: Option<magnus::RHash>) -> Option<Value> {
    let h = kw?;
    h.get(ruby.to_symbol("context"))
}

/* ---- Document.parse ---- */

/// Arguments for the GVL-released parse.
#[repr(C)]
struct ParseArgs {
    src: *const u8,
    len: usize,
    assume_valid: bool,
    result: *mut c_void,
}

/// Runs with the GVL released: pure C (Lexbor + libc), touching no Ruby state.
unsafe extern "C" fn parse_nogvl(p: *mut c_void) -> *mut c_void {
    let a = &mut *(p as *mut ParseArgs);
    a.result = mkr_parse_html(a.src, a.len, a.assume_valid);
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
        let src = mkr_ruby_to_utf8(s.as_raw());

        /* Copy the source out BEFORE allocating the wrapper. Allocating is a GC
         * point, and a borrowed pointer into a Ruby String's backing store must
         * not straddle one - nor be held while the GVL is released. The
         * coderange is read first (no scan): a source Ruby already knows is
         * valid UTF-8 lets the parse skip its sanitisation. */
        let assume_valid = mkr_ruby_str_known_valid_utf8(src);
        let mut owned = OwnedBytes { ptr: core::ptr::null_mut(), len: 0 };
        if mkr_ruby_copy_bytes(src, &mut owned) != 0 {
            return Err(Error::new(error_class(), "out of memory copying source"));
        }

        /* Allocate the wrapper with a null handle, so a failed parse still
         * frees cleanly through GC. This entry is defined on
         * Makiri::HTML::Document, so the result is always HTML. */
        let d = rb_sys::ruby_xcalloc(1, core::mem::size_of::<DocData>() as rb_sys::size_t)
            as *mut DocData;
        (*d).parsed = core::ptr::null_mut();
        (*d).errors = rb_sys::rb_ary_new();
        let obj = rb_sys::rb_data_typed_object_wrap(
            klass.as_raw(),
            d as *mut c_void,
            MKR_HTML_DOC_TYPE.as_ptr(),
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
        owned_bytes_clear(&mut owned);

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
        let doc = mkr_html_doc_unwrap(self_.as_raw());
        Value::from_raw(mkr_wrap_html_node(lxb_dom_document_root(doc), self_.as_raw()))
    }
}

/// The document `<title>`, or `""`.
fn doc_title(ruby: &Ruby, self_: Value) -> RString {
    unsafe {
        let mut len: usize = 0;
        let doc = mkr_html_doc_unwrap(self_.as_raw());
        let s = lxb_html_document_title(doc as *mut c_void, &mut len);
        let bytes: &[u8] = if s.is_null() { &[] } else { core::slice::from_raw_parts(s, len) };
        ruby.enc_str_new(bytes, ruby.utf8_encoding())
    }
}

/// The `<!DOCTYPE ...>` node, or nil - Nokogiri's `#internal_subset`. It is a
/// child of the document node (typically first), so a short scan finds it.
fn doc_internal_subset(ruby: &Ruby, self_: Value) -> Value {
    unsafe {
        let doc = mkr_html_doc_unwrap(self_.as_raw()) as *mut LxbNode;
        let mut c = (*doc).first_child;
        while !c.is_null() {
            if (*c).type_ == NODE_TYPE_DOCUMENT_TYPE {
                return Value::from_raw(mkr_wrap_html_node(c, self_.as_raw()));
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
        let doc = mkr_html_doc_unwrap(self_.as_raw());
        Value::from_raw(rb_sys::rb_int2inum((*doc).compat_mode as isize))
    }
}

/// Parse warnings. Reserved; currently always empty.
fn doc_errors(ruby: &Ruby, self_: Value) -> Value {
    let _ = ruby;
    unsafe {
        let d = rb_sys::rb_check_typeddata(self_.as_raw(), mkr_doc_type.as_ptr()) as *mut DocData;
        Value::from_raw((*d).errors)
    }
}

/* ---- fragment entry points ---- */

/// `document.fragment(html, context: ...)` -> a DocumentFragment bound to this
/// document. `context` defaults to `<body>`.
fn doc_fragment(ruby: &Ruby, self_: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let (html,) = a.required;
    let context = context_kwarg(ruby, Some(a.keywords));
    unsafe {
        let (tag, ns) = resolve_fragment_context(mkr_html_doc_unwrap(self_.as_raw()), context);
        build_fragment_ctx(ruby, self_, html, tag, ns)
    }
}

/// `DocumentFragment.parse(html, context: ...)` -> a standalone fragment with
/// its own backing document, kept alive by the fragment's wrapper.
fn frag_s_parse(ruby: &Ruby, _klass: Value, args: &[Value]) -> Result<Value, Error> {
    let a = magnus::scan_args::scan_args::<(Value,), (), (), (), magnus::RHash, ()>(args)?;
    let (html,) = a.required;
    let context = context_kwarg(ruby, Some(a.keywords));
    unsafe {
        const SHELL: &[u8] = b"<html><body></body></html>";
        let parsed = mkr_parse_html(SHELL.as_ptr(), SHELL.len(), true);
        if parsed.is_null() {
            return Err(Error::new(error_class(), "failed to create fragment document"));
        }
        let document = Value::from_raw(mkr_wrap_document(parsed)); /* GC owns parsed now */
        let (tag, ns) = resolve_fragment_context(mkr_html_doc_unwrap(document.as_raw()), context);
        build_fragment_ctx(ruby, document, html, tag, ns)
    }
}

/// `node.parse(html)` -> a NodeSet of nodes parsed as a fragment in this
/// element's context. Nokogiri-compatible, and the way to reach a foreign
/// (SVG/MathML) fragment context.
fn node_parse(ruby: &Ruby, self_: Value, rb_html: Value) -> Result<Value, Error> {
    unsafe {
        let node = mkr_html_node_unwrap(self_.as_raw());
        if (*node).type_ != NODE_TYPE_ELEMENT {
            return Err(Error::new(
                ruby.exception_arg_error(),
                "Node#parse requires an element context",
            ));
        }
        let document = Value::from_raw(mkr_node_document(self_.as_raw()));
        let frag = build_fragment_ctx(ruby, document, rb_html, (*node).local_name, (*node).ns)?;
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
        let doc = mkr_html_doc_unwrap(self_.as_raw());

        /* An XML node is TRANSLATED across representations (mkr -> lxb) into a
         * detached lxb subtree owned by this document. */
        if mkr_node_kind(node_v.as_raw()) == MKR_NODE_KIND_XML {
            let mut imp: *mut LxbNode = core::ptr::null_mut();
            mkr_xml_mut_check(mkr_cross_xml_to_html(
                doc,
                mkr_xml_node_unwrap(node_v.as_raw()),
                deep,
                &mut imp,
            ));
            return Ok(Value::from_raw(mkr_wrap_html_node(imp, self_.as_raw())));
        }

        let src = mkr_html_node_unwrap(node_v.as_raw()); /* raises on a non-node */
        let imp = lxb_dom_document_import_node(doc, src, deep);
        if imp.is_null() {
            return Err(Error::new(error_class(), "failed to import node"));
        }
        if deep {
            fixup_template_content(doc, src, imp);
        }
        Ok(Value::from_raw(mkr_wrap_html_node(imp, self_.as_raw())))
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
#[no_mangle]
pub unsafe extern "C" fn mkr_node_clone_node(
    argc: c_int,
    argv: *const VALUE,
    self_: VALUE,
) -> VALUE {
    let mut deep_v: VALUE = rb_sys::Qnil as VALUE;
    rb_sys::rb_scan_args(argc, argv, c"01".as_ptr(), &mut deep_v);
    /* RTEST: anything but nil and false. */
    let deep = deep_v != rb_sys::Qnil as VALUE && deep_v != rb_sys::Qfalse as VALUE;

    let node = mkr_html_node_unwrap(self_);
    let doc = (*node).owner_document;

    let clone = lxb_dom_document_import_node(doc, node, deep);
    if clone.is_null() {
        super::abi::rb_raise(super::abi::mkr_eError, c"failed to clone node".as_ptr());
    }
    if deep {
        fixup_template_content(doc, node, clone);
    }
    mkr_wrap_html_node(clone, mkr_node_document(self_))
}

/* ---- registration ---- */

/// `Init_makiri` calls this where it called the C one.
///
/// # Safety
/// Runs once, from `Init_makiri`, on the Ruby thread.
#[no_mangle]
pub unsafe extern "C" fn mkr_init_document() {
    let ruby = Ruby::get().expect("mkr_init_document runs on the Ruby thread");
    let html_doc = magnus::RClass::from_value(Value::from_raw(mkr_cHtmlDocument))
        .expect("Makiri::HTML::Document is a class");

    html_doc
        .define_singleton_method("_parse", method!(doc_s_parse, 1))
        .expect("Document._parse");
    html_doc.define_method("root", method!(doc_root, 0)).expect("Document#root");
    html_doc.define_method("title", method!(doc_title, 0)).expect("Document#title");
    html_doc.define_method("errors", method!(doc_errors, 0)).expect("Document#errors");
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
