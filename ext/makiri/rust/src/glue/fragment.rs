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

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use magnus::rb_sys::{AsRawValue, FromRawValue};
use magnus::{prelude::*, Error, Ruby, Value};
use rb_sys::VALUE;

use crate::falloc::VecPush;
use crate::lexbor_abi as lxb;

use super::abi::{
    error_class, html_node_unwrap, is_kind_of, ruby_bytes_view, ruby_str_known_valid_utf8,
    ruby_to_utf8, ruby_verified_text, wrap_html_node, LxbDoc, LxbNode, CLASS_NODE,
    LXB_DOM_NODE_TYPE_ELEMENT,
};

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

use crate::cbuf::{Buf, OwnedBuf};
use crate::dom_adapter::html::{BuildingNode, HtmlDoc, HtmlNode};
pub use crate::dom_adapter::utf8_input::utf8_sanitize;
use crate::dom_adapter::utf8_input::Sanitized;

/* `import_node` and the two inserts are generated; they were declared here as
 * well, which is the one-symbol-two-declarations hazard `lexbor_abi` exists to
 * prevent - `mutate.rs` reached the same three through the generated bindings.
 * The two Lexbor leaves out of its public headers live in `lexbor_abi` with the
 * rest of what bindgen cannot see. */
use crate::lexbor_abi::{
    lxb_dom_document_fragment_interface_create, lxb_dom_document_import_node,
    lxb_dom_node_insert_before, lxb_dom_node_insert_child, lxb_html_parse_fragment_by_tag_id,
};

/* The HTML parser's lifecycle, from the generated bindings. Declared here first
 * over an opaque parser, which was fine until the source-location port needed
 * the tokenizer inside it and build.rs started generating them - two Rust types
 * for one symbol again. */
use crate::lexbor_abi::{
    /* The `_noi` twin of an `lxb_inline`. It was declared here, over an opaque
     * hash, until the HTML shim needed the same symbol - one declaration per
     * symbol, and `lexbor_abi` is where the `_noi` twins live. */
    lxb_tag_id_by_name_noi, HtmlParser,
};

/// Lexbor node types and the tag/namespace ids this file compares against.
/// Generated, so a pin that renumbers them is a build-time change, not a
/// silently different answer (see lexbor_abi).
const NS_HTML: usize = lxb::lxb_ns_id_enum_t_LXB_NS_HTML as usize;
const NS_SVG: usize = lxb::lxb_ns_id_enum_t_LXB_NS_SVG as usize;
const NS_MATH: usize = lxb::lxb_ns_id_enum_t_LXB_NS_MATH as usize;

/// `lxb_dom_document_import_node` deep-clones the normal child chain but NOT a
/// `<template>`'s separate content fragment, so an imported template comes out
/// empty. Walk source and clone in lockstep (deep import preserves child order
/// 1:1) and import each template's content into the clone's.
///
/// Iterative, with an explicit worklist: an adversarially deep fragment must not
/// be able to overflow the stack. Best-effort on allocation failure, as the C
/// was - a template whose content could not be copied is left empty rather than
/// the whole import failing.
///
/// `template_content` answers in one what this used to ask in three: it is
/// `None` for a node that is not an HTML `<template>` AND for one Lexbor gave no
/// contents fragment. Those are the same case here, because the only thing this
/// walk does is copy one existing contents fragment into another - which is why
/// `cross_import`'s `h2x_children_of`, where an empty template and a
/// non-template mean DIFFERENT children, keeps a test of its own.
fn fixup_template_content(
    doc: HtmlDoc<'_>,
    root_src: HtmlNode<'_>,
    root_clone: BuildingNode<'_>,
) -> Result<(), ()> {
    let mut stack: Vec<(HtmlNode<'_>, BuildingNode<'_>)> = Vec::new();
    stack.mkr_push((root_src, root_clone))?;

    while let Some((src_root, clone_root)) = stack.pop() {
        let (mut sn, mut cn) = (Some(src_root), Some(clone_root));
        while let (Some(s), Some(c)) = (sn, cn) {
            /* Nested rather than a tuple: the clone-side test is only worth
             * paying for once the source side has said this is a template, and
             * every node of every deep import passes through here. */
            if let Some((sc, cc)) = s
                .template_content()
                .and_then(|sc| Some((sc, c.template_content()?)))
            {
                let mut x = sc.first_child();
                while let Some(child) = x {
                    /* SAFETY: a live document and a live source node; what
                     * import returns is fresh and detached, which is what
                     * BuildingNode means. */
                    let imp = unsafe {
                        BuildingNode::from_raw(lxb_dom_document_import_node(
                            doc.as_raw(),
                            child.as_raw(),
                            true,
                        ))
                    };
                    let Some(imp) = imp else {
                        // Lexbor could not copy a content child. Giving up
                        // here leaves the clone's template SHORT, which is
                        // the truncated answer the contract forbids.
                        return Err(());
                    };
                    cc.insert_child(imp);
                    x = child.next();
                }
                stack.mkr_push((sc, cc))?;
            }
            sn = s.preorder_next(src_root);
            cn = c.preorder_next(clone_root);
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
    let clean = match utf8_sanitize(hv.as_ptr() as *const u8, hv.len()) {
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

pub unsafe extern "C" fn emit_append(imported: *mut LxbNode, u: *mut c_void) {
    lxb_dom_node_insert_child(u as *mut LxbNode, imported);
}

pub unsafe extern "C" fn emit_before(imported: *mut LxbNode, u: *mut c_void) {
    lxb_dom_node_insert_before(u as *mut LxbNode, imported);
}

/// Deep-import each child of `root` into `doc` and hand it to `emit`.
///
/// `-1` when a child could not be copied whole. It RETURNS rather than raising:
/// `ruby_html_mutate.c` destroys a transient fragment document after this call,
/// and a longjmp past that free leaks one Lexbor document per failure - the leak
/// that free was added to fix. The caller raises once its own cleanup has run.
pub unsafe extern "C" fn import_fragment_children(
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

/// Run a fragment parse with a fresh parser, or the error that stopped it.
///
/// The parser is destroyed on every path: the fragment tree belongs to its
/// document, not the parser, so it survives - the caller may still read
/// `root->owner_document` afterwards.
pub unsafe fn run_fragment_parser(
    html: VALUE,
    parse: FragmentParseFn,
    ctx: *mut c_void,
) -> Result<*mut LxbNode, Error> {
    let Some(parser) = HtmlParser::create() else {
        return Err(Error::new(error_class(), "failed to create HTML parser"));
    };

    let Some(src) = sanitize_html_input(html) else {
        return Err(Error::new(
            error_class(),
            "out of memory decoding fragment HTML",
        ));
    };

    /* The callback contract is representation-opaque (it is a C function
     * pointer handed across the boundary), so the typed parser is cast here
     * rather than declared a second time. */
    let root = parse(parser.as_ptr() as *mut c_void, src.ptr, src.len, ctx);
    drop(src); /* the parse consumed it; the buffer goes on every path */
    drop(parser); /* the fragment belongs to its document, not to the parser */
    if root.is_null() {
        return Err(Error::new(error_class(), "failed to parse HTML fragment"));
    }
    Ok(root)
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
    /* SAFETY: a live document, the caller's live source node, and the copy
     * just made - detached, and nothing outside it points at it yet. The walk
     * is the only thing that reads them, and it does not outlive this call. */
    let handles = (
        HtmlDoc::from_raw(doc),
        HtmlNode::from_raw(src),
        BuildingNode::from_raw(imp),
    );
    let (Some(hdoc), Some(hsrc), Some(himp)) = handles else {
        return None;
    };
    if deep && fixup_template_content(hdoc, hsrc, himp).is_err() {
        // A copy whose <template> lost its contents is a wrong answer, not a
        // degraded one: `<template><i>x</i></template>` comes back as
        // `<template></template>` and nothing says so. The C was best-effort
        // here and the port carried that over; the OOM sweep called it, which
        // is what the sweep is for.
        return None;
    }
    Some(imp)
}

/// Deep-import `src` into `doc`, or an error rather than a partial node.
pub unsafe fn html_import_deep(doc: *mut LxbDoc, src: *mut LxbNode) -> Result<*mut LxbNode, Error> {
    import_with_fixup(doc, src, true)
        .ok_or_else(|| Error::new(error_class(), "failed to import node"))
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

    if is_kind_of(context, &CLASS_NODE) {
        /* Reject an XML node before any Lexbor use. */
        let cn = html_node_unwrap(context)?;
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
    let cv = ruby_verified_text(context, c"fragment context element")?;
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

/// Parse callback for `run_fragment_parser`: Lexbor's by-tag-id parser,
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
    let root = run_fragment_parser(
        html.as_raw(),
        parse_fragment_by_tag,
        &pctx as *const FragTagCtx as *mut c_void,
    )?;
    if import_fragment_children(doc, root, emit_append, frag_node as *mut c_void) != 0 {
        return Err(Error::new(
            error_class(),
            "failed to import a fragment child",
        ));
    }
    let out = wrap_html_node(frag_node, document.as_raw());
    Ok(Value::from_raw(out))
}

/// The `context:` keyword, or None.
pub fn context_kwarg(ruby: &Ruby, kw: Option<magnus::RHash>) -> Option<Value> {
    let h = kw?;
    h.get(ruby.to_symbol("context"))
}
