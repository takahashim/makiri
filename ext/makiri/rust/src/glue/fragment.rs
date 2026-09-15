//! The HTML fragment pipeline (was part of glue/ruby_doc.c).
//!
//! Parsing a fragment, importing its children into a document, and the
//! `<template>`-content fixup that `lxb_dom_document_import_node` omits. Five
//! of these are exported C symbols, called by `ruby_html_mutate.c` and
//! `cross_import.c`.
//!
//! # Why this is not in `doc.rs`
//!
//! The C had one file because the C had one file. None of this is about the
//! Document WRAPPER - it is a service the wrapper happens to use and two other
//! translation units use directly, and keeping it here means the module that
//! owns `Makiri::HTML::Document` is about that class rather than about three
//! unrelated things.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, Ruby, Value};
use rb_sys::VALUE;

use crate::falloc::VecPush;
use crate::lexbor_abi as lxb;

use super::abi::{
    error_class, is_kind_of, mkr_cNode, mkr_html_node_unwrap, mkr_wrap_html_node, ruby_bytes_view,
    ruby_str_known_valid_utf8, ruby_to_utf8, ruby_verified_text, LxbDoc, LxbNode,
    LXB_DOM_NODE_TYPE_ELEMENT,
};

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

use crate::cbuf::{Buf, OwnedBuf};
pub use crate::dom_adapter::utf8_input::mkr_utf8_sanitize;
use crate::dom_adapter::utf8_input::Sanitized;

extern "C" {

    fn lxb_html_parse_fragment_by_tag_id(
        parser: *mut c_void,
        doc: *mut c_void,
        tag: usize,
        ns: usize,
        src: *const u8,
        len: usize,
    ) -> *mut LxbNode;
    fn lxb_dom_document_fragment_interface_create(doc: *mut LxbDoc) -> *mut c_void;
    fn lxb_dom_document_import_node(
        doc: *mut LxbDoc,
        node: *mut LxbNode,
        deep: bool,
    ) -> *mut LxbNode;
    fn lxb_dom_node_insert_child(to: *mut LxbNode, node: *mut LxbNode);
    fn lxb_dom_node_insert_before(to: *mut LxbNode, node: *mut LxbNode);
}

/* The HTML parser's lifecycle, from the generated bindings. Declared here first
 * over an opaque parser, which was fine until the source-location port needed
 * the tokenizer inside it and build.rs started generating them - two Rust types
 * for one symbol again. */
use crate::glue::abi::LXB_STATUS_OK;
use crate::lexbor_abi::{
    lxb_html_parser_create, lxb_html_parser_destroy, lxb_html_parser_init,
    /* The `_noi` twin of an `lxb_inline`. It was declared here, over an opaque
     * hash, until the HTML shim needed the same symbol - one declaration per
     * symbol, and `lexbor_abi` is where the `_noi` twins live. */
    lxb_tag_id_by_name_noi,
};

/// The shared pre-order walk. Defined once in `lexbor_abi` - it was written out
/// here first, and the text-index port would have been a second copy of an
/// invariant that must not drift.
use crate::lexbor_abi::preorder_next;

/// Lexbor node types and the tag/namespace ids this file compares against.
/// Generated, so a pin that renumbers them is a build-time change, not a
/// silently different answer (see lexbor_abi).
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
    (*n).type_ == LXB_DOM_NODE_TYPE_ELEMENT
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
unsafe fn fixup_template_content(
    doc: *mut LxbDoc,
    root_src: *mut LxbNode,
    root_clone: *mut LxbNode,
) -> Result<(), ()> {
    let mut stack: Vec<(*mut LxbNode, *mut LxbNode)> = Vec::new();
    stack.mkr_push((root_src, root_clone))?;

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
                        if imp.is_null() {
                            // Lexbor could not copy a content child. Giving up
                            // here leaves the clone's template SHORT, which is
                            // the truncated answer the contract forbids.
                            return Err(());
                        }
                        lxb_dom_node_insert_child(cc, imp);
                        x = (*x).next;
                    }
                    stack.mkr_push((sc, cc))?;
                }
            }
            sn = preorder_next(sn, src_root);
            cn = preorder_next(cn, clone_root);
        }
    }
    Ok(())
}

/// What `sanitize_html_input` decided about the input bytes.
///
/// The buffer is `Some` when the bytes are OURS to free and `None` when they
/// are borrowed from the Ruby String the caller is keeping alive - the
/// distinction the C expressed with an out-parameter that the caller had to
/// remember to `free`.
pub struct SanitizedHtml {
    pub ptr: *const u8,
    pub len: usize,
    _owned: Option<OwnedBuf>,
}

/// Browser-compatible decoding for fragment input: invalid UTF-8 becomes
/// U+FFFD, valid input is used in place. `None` on OOM with nothing allocated.
///
/// This is the Rust-facing form. The C ABI wrapper below hands the same result
/// back through four out-parameters because that is what `ruby_html_mutate.c`
/// expects; in Rust the ownership is in the type and `Drop` frees it, so no
/// caller has to remember.
pub unsafe fn sanitize_html_input(html: VALUE) -> Option<SanitizedHtml> {
    let u8v = ruby_to_utf8(html);
    let hv = ruby_bytes_view(u8v);

    if u8v != html {
        // Transcoded: a fresh String nothing keeps alive past this return, so
        // its bytes must NOT be borrowed. It is already valid UTF-8, so copy
        // rather than sanitise.
        let mut buf = Buf::new(hv.len());
        buf.append(if hv.len() == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(hv.as_ptr() as *const u8, hv.len())
        })
        .ok()?;
        let owned = buf.steal().ok()?;
        let ptr = owned.as_slice().as_ptr();
        let len = owned.as_slice().len();
        return Some(SanitizedHtml {
            ptr,
            len,
            _owned: Some(owned),
        });
    }

    // Not transcoded: input Ruby already knows is valid UTF-8 is borrowed in
    // place (the caller keeps `html` alive); anything else is sanitised.
    if ruby_str_known_valid_utf8(html) {
        return Some(SanitizedHtml {
            ptr: hv.as_ptr() as *const u8,
            len: hv.len(),
            _owned: None,
        });
    }
    let clean = match mkr_utf8_sanitize(hv.as_ptr() as *const u8, hv.len()) {
        Some(Sanitized::Unchanged) => None,
        Some(Sanitized::Replaced(r)) => Some(r),
        None => return None,
    };
    match clean {
        None => Some(SanitizedHtml {
            ptr: hv.as_ptr() as *const u8,
            len: hv.len(),
            _owned: None,
        }),
        Some(r) => {
            let ptr = r.as_slice().as_ptr();
            let len = r.as_slice().len();
            Some(SanitizedHtml {
                ptr,
                len,
                _owned: Some(r),
            })
        }
    }
}

pub unsafe extern "C" fn mkr_emit_append(imported: *mut LxbNode, u: *mut c_void) {
    lxb_dom_node_insert_child(u as *mut LxbNode, imported);
}

pub unsafe extern "C" fn mkr_emit_before(imported: *mut LxbNode, u: *mut c_void) {
    lxb_dom_node_insert_before(u as *mut LxbNode, imported);
}

/// Deep-import each child of `root` into `doc` and hand it to `emit`.
///
/// `-1` when a child could not be copied whole. It RETURNS rather than raising:
/// `ruby_html_mutate.c` destroys a transient fragment document after this call,
/// and a longjmp past that free leaks one Lexbor document per failure - the leak
/// that free was added to fix. The caller raises once its own cleanup has run.
pub unsafe extern "C" fn mkr_import_fragment_children(
    doc: *mut LxbDoc,
    root: *mut LxbNode,
    emit: unsafe extern "C" fn(*mut LxbNode, *mut c_void),
    u: *mut c_void,
) -> c_int {
    let mut f = (*root).first_child;
    while !f.is_null() {
        let next = (*f).next; /* import does not unlink f, but be safe */
        match import_with_fixup(doc, f, true) {
            Some(imp) => emit(imp, u),
            None => return -1,
        }
        f = next;
    }
    0
}

/// `mkr_fragment_parse_fn`.
type FragmentParseFn =
    unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut c_void) -> *mut LxbNode;

/// Run a fragment parse with a fresh parser. **Raises** on failure.
///
/// The parser is destroyed before any raise: the fragment tree belongs to its
/// document, not the parser, so it survives - the caller may still read
/// `root->owner_document` afterwards.
pub unsafe extern "C" fn mkr_run_fragment_parser(
    html: VALUE,
    parse: FragmentParseFn,
    ctx: *mut c_void,
) -> *mut LxbNode {
    let parser = lxb_html_parser_create();
    if parser.is_null() || lxb_html_parser_init(parser) != LXB_STATUS_OK {
        if !parser.is_null() {
            lxb_html_parser_destroy(parser);
        }
        super::abi::rb_raise(
            super::abi::mkr_eError,
            c"failed to create HTML parser".as_ptr(),
        );
    }

    let Some(src) = sanitize_html_input(html) else {
        lxb_html_parser_destroy(parser);
        super::abi::rb_raise(
            super::abi::mkr_eError,
            c"out of memory decoding fragment HTML".as_ptr(),
        );
    };

    /* The callback contract is representation-opaque (it is a C function
     * pointer handed across the boundary), so the typed parser is cast here
     * rather than declared a second time. */
    let root = parse(parser as *mut c_void, src.ptr, src.len, ctx);
    drop(src); /* the parse consumed it; the buffer goes on every path */
    lxb_html_parser_destroy(parser);
    if root.is_null() {
        super::abi::rb_raise(
            super::abi::mkr_eError,
            c"failed to parse HTML fragment".as_ptr(),
        );
    }
    root
}

/// Copy `src` into `doc`, `<template>` contents included, or `None` on failure.
///
/// The DOM `importNode` omits a template's separate content fragment, so every
/// copy in this extension is import-plus-fixup; this is that one operation.
/// Three callers wanted it with different deep flags, different error channels
/// and different messages, and each had grown its own copy of the four lines -
/// so the operation lives here and they keep only the parts that differ.
pub unsafe fn import_with_fixup(
    doc: *mut LxbDoc,
    src: *mut LxbNode,
    deep: bool,
) -> Option<*mut LxbNode> {
    let imp = lxb_dom_document_import_node(doc, src, deep);
    if imp.is_null() {
        return None;
    }
    if deep && fixup_template_content(doc, src, imp).is_err() {
        // A copy whose <template> lost its contents is a wrong answer, not a
        // degraded one: `<template><i>x</i></template>` comes back as
        // `<template></template>` and nothing says so. The C was best-effort
        // here and the port carried that over; the OOM sweep called it, which
        // is what the sweep is for.
        return None;
    }
    Some(imp)
}

/// Deep-import `src` into `doc`. **Raises** rather than returning a partial
/// node. The C ABI face of [`import_with_fixup`], called by
/// `ruby_html_mutate.c`.
pub unsafe extern "C" fn mkr_html_import_deep(doc: *mut LxbDoc, src: *mut LxbNode) -> *mut LxbNode {
    match import_with_fixup(doc, src, true) {
        Some(imp) => imp,
        None => super::abi::rb_raise(super::abi::mkr_eError, c"failed to import node".as_ptr()),
    }
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
/// `Err` for an unusable context.
pub unsafe fn resolve_fragment_context(
    doc: *mut LxbDoc,
    context: Option<Value>,
) -> Result<(usize, usize), magnus::Error> {
    let Some(context) = context else {
        return Ok((lxb::lxb_tag_id_enum_t_LXB_TAG_BODY as usize, NS_HTML));
    };
    if context.is_nil() {
        return Ok((lxb::lxb_tag_id_enum_t_LXB_TAG_BODY as usize, NS_HTML));
    }

    if is_kind_of(context, mkr_cNode) {
        /* Reject an XML node before any Lexbor use. */
        let cn = mkr_html_node_unwrap(context.as_raw())?;
        if (*cn).type_ != LXB_DOM_NODE_TYPE_ELEMENT {
            return Err(magnus::Error::new(
                magnus::Ruby::get_unchecked().exception_arg_error(),
                "fragment context node must be an element",
            ));
        }
        return Ok(((*cn).local_name, (*cn).ns));
    }

    /* A context tag name is a programmatic control string, not parsed HTML, so
     * it follows the strict text-input contract (valid UTF-8, no NUL). */
    let cv = ruby_verified_text(context.as_raw(), c"fragment context element".as_ptr())?;
    let name = if cv.as_ptr().is_null() || cv.len() == 0 {
        &[][..]
    } else {
        core::slice::from_raw_parts(cv.as_ptr() as *const u8, cv.len())
    };
    if name == b"svg" {
        return Ok((lxb::lxb_tag_id_enum_t_LXB_TAG_SVG as usize, NS_SVG));
    }
    if name == b"math" {
        return Ok((lxb::lxb_tag_id_enum_t_LXB_TAG_MATH as usize, NS_MATH));
    }
    let tid = lxb_tag_id_by_name_noi((*doc).tags, name.as_ptr(), name.len());
    if tid == lxb::lxb_tag_id_enum_t_LXB_TAG__UNDEF as usize {
        // The C wrote `"...: %" PRIsVALUE` - two string literals the C
        // preprocessor joins. Rust has no such concatenation, so carrying the
        // line over verbatim produced the literal `%" PRIsVALUE` in the
        // message; the differential caught it. `%.*s` over the verified bytes
        // prints what PRIsVALUE printed for a String: its content.
        /* The name is verified UTF-8, so the lossy view is its content. */
        return Err(magnus::Error::new(
            magnus::Ruby::get_unchecked().exception_arg_error(),
            format!(
                "unknown fragment context element: {}",
                String::from_utf8_lossy(name)
            ),
        ));
    }
    Ok((tid, NS_HTML))
}

/// Parse callback for `mkr_run_fragment_parser`: Lexbor's by-tag-id parser,
/// which implements the full algorithm for the context (tokenizer state for
/// rawtext/rcdata, foreign-content adjustment, the form pointer).
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
/// `document` is the wrapper the fragment is bound to (its keepalive), `doc` the
/// Lexbor document already unwrapped from it.
///
/// Taking both, rather than unwrapping here, is what keeps this module from
/// depending on `glue::doc` - unwrapping a Document is that module's job, and
/// the import back the other way made the two mutually dependent.
pub unsafe fn build_fragment_ctx(
    ruby: &Ruby,
    document: Value,
    doc: *mut LxbDoc,
    rb_html: Value,
    tag: usize,
    ns: usize,
) -> Result<Value, Error> {
    let html = ruby.into_value(rb_html.to_r_string()?);

    let frag = lxb_dom_document_fragment_interface_create(doc);
    if frag.is_null() {
        return Err(Error::new(
            error_class(),
            "failed to create document fragment",
        ));
    }
    let frag_node = frag as *mut LxbNode;

    let pctx = FragTagCtx { doc, tag, ns };
    let root = mkr_run_fragment_parser(
        html.as_raw(),
        parse_fragment_by_tag,
        &pctx as *const FragTagCtx as *mut c_void,
    );
    if mkr_import_fragment_children(doc, root, mkr_emit_append, frag_node as *mut c_void) != 0 {
        return Err(Error::new(
            error_class(),
            "failed to import a fragment child",
        ));
    }
    let out = mkr_wrap_html_node(frag_node, document.as_raw());
    Ok(Value::from_raw(out))
}

/// The `context:` keyword, or None.
pub fn context_kwarg(ruby: &Ruby, kw: Option<magnus::RHash>) -> Option<Value> {
    let h = kw?;
    h.get(ruby.to_symbol("context"))
}
