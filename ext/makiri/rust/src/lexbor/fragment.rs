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

use core::ffi::c_void;

use magnus::rb_sys::AsRawValue;
use magnus::{prelude::*, Error, Ruby, Value};
use crate::bridge::ruby::VALUE;

use crate::falloc::VecPush;

use crate::glue::abi::{
    error_class, html_node_unwrap, is_kind_of, ruby_bytes_view, ruby_str_known_valid_utf8,
    ruby_to_utf8, ruby_verified_text, wrap_html_node,
};
use crate::lexbor::adapter::html::TYPE_ELEMENT as LXB_DOM_NODE_TYPE_ELEMENT;
use crate::lexbor::ffi::{LxbDoc, LxbNode};
use crate::init::CLASS_NODE;

/* ------------------------------------------------------------------ *
 * fragments                                                          *
 * ------------------------------------------------------------------ */

use crate::cbuf::{Buf, OwnedBuf};
use crate::lexbor::adapter::html::{
    BuildingNode, HtmlDoc, HtmlNode, RawDoc, RawNode, NS_HTML, NS_MATH, NS_SVG, TAG_BODY, TAG_MATH,
    TAG_SVG, TAG_UNDEF,
};
pub use crate::lexbor::adapter::utf8_input::utf8_sanitize;
use crate::lexbor::adapter::utf8_input::Sanitized;

/* The two fragment parsers. One is generated; the other is exported by Lexbor
 * but absent from its public headers, so `lexbor_abi` hand-declares it with the
 * rest of what bindgen cannot see. Everything this file does to the DOM itself
 * goes through `lexbor::adapter::html` - these are the parser, not the DOM. */
use crate::lexbor_abi::{lxb_html_parse_fragment, lxb_html_parse_fragment_by_tag_id};

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
                    let imp = doc.import_node(child, true);
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
        buf.append(hv.bytes()).ok()?;
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

/// Where [`import_fragment_children`] puts each child it imports.
///
/// The node used to travel as a `*mut c_void` beside a function pointer, which
/// left nothing connecting the two: the callback cast that pointer to a node,
/// so handing it something else was a cast away and no diagnostic. Choice and
/// data are one value now.
pub enum Emit {
    /// As the last child of this node.
    Append(RawNode),
    /// Immediately before this node, under its parent.
    Before(RawNode),
}

impl Emit {
    /// # Safety
    /// The node must be live, and the caller must be clear to change the tree
    /// it belongs to.
    unsafe fn put(&self, imported: RawNode) {
        /* SAFETY: both are live nodes of one document, still being built -
         * which is what this type's variants carry and what import just made. */
        let Some(imported) = BuildingNode::from_raw(imported.as_ptr() as *mut LxbNode) else {
            return;
        };
        match *self {
            Emit::Append(at) => {
                if let Some(at) = BuildingNode::from_raw(at.as_ptr() as *mut LxbNode) {
                    at.insert_child(imported);
                }
            }
            Emit::Before(at) => {
                if let Some(at) = BuildingNode::from_raw(at.as_ptr() as *mut LxbNode) {
                    at.insert_before(imported);
                }
            }
        }
    }
}

/// Deep-import each child of `root` into `doc` and place it per `emit`.
///
/// `false` when a child could not be copied whole. It REPORTS rather than
/// raising, and that still matters now the C has gone: every caller owns the
/// transient document the fragment was parsed into
/// (`lexbor_abi::TransientDoc`), and a raise from here would longjmp past its
/// `Drop` - one leaked Lexbor document per failure. The caller raises once its
/// own cleanup has run, with the message that suits it.
pub unsafe fn import_fragment_children(doc: RawDoc, root: RawNode, emit: &Emit) -> bool {
    let mut f = (*(root.as_ptr() as *mut LxbNode)).first_child;
    while !f.is_null() {
        let next = (*f).next; /* import does not unlink f, but be safe */
        match import_raw(doc, f, true) {
            Some(imp) => emit.put(RawNode::from_ptr(imp.cast()).expect("imported child")),
            None => return false,
        }
        f = next;
    }
    true
}

/// Import a fragment parsed in Lexbor's transient document, then release that
/// document on every return path.  The transient-document ownership rule is a
/// Lexbor ABI concern, so callers never need to name `TransientDoc`.
pub unsafe fn import_transient_fragment_children(
    doc: RawDoc,
    root: RawNode,
    emit: &Emit,
) -> bool {
    let _transient = crate::lexbor_abi::TransientDoc::of(root.as_ptr() as *mut LxbNode);
    import_fragment_children(doc, root, emit)
}

/// Which Lexbor fragment parser to run, and the context it needs.
///
/// Both implement the same WHATWG algorithm - tokenizer state for
/// rawtext/rcdata, foreign-content adjustment, the form pointer - and differ
/// only in how the context arrives. The context used to be a `*mut c_void`
/// beside a function pointer, cast back by whichever callback was chosen; it
/// travels with the choice now.
pub enum FragmentContext {
    /// The context element itself, which `inner_html=` and `outer_html=` have.
    Element(RawNode),
    /// A named context: a tag id and namespace, for `Document#fragment` and
    /// `DocumentFragment.parse`, where no such element exists yet.
    Tag {
        doc: RawDoc,
        tag: usize,
        ns: usize,
    },
}

impl FragmentContext {
    /// # Safety
    /// `parser` must be live and initialised, and the source bytes must stay
    /// put for the call. The context - element or document - must be live.
    unsafe fn parse(&self, parser: &HtmlParser, src: *const u8, len: usize) -> *mut LxbNode {
        match *self {
            /* Lexbor types this one to its element interface; the handle we
             * hold is a node, which is what that interface begins with. */
            FragmentContext::Element(el) => {
                lxb_html_parse_fragment(parser.as_ptr(), el.as_ptr() as *mut _, src, len)
            }
            /* The by-tag-id entry is hand-declared over opaque pointers (it is
             * absent from Lexbor's public headers), so the casts are here. */
            FragmentContext::Tag { doc, tag, ns } => lxb_html_parse_fragment_by_tag_id(
                parser.as_ptr() as *mut c_void,
                doc.as_ptr(),
                tag,
                ns,
                src,
                len,
            ),
        }
    }
}

/// Run a fragment parse with a fresh parser, or the error that stopped it.
///
/// The parser is destroyed on every path: the fragment tree belongs to its
/// document, not the parser, so it survives - the caller may still read
/// `root->owner_document` afterwards.
pub unsafe fn run_fragment_parser(
    html: VALUE,
    context: &FragmentContext,
) -> Result<RawNode, Error> {
    let Some(parser) = HtmlParser::create() else {
        return Err(Error::new(error_class(), "failed to create HTML parser"));
    };

    let Some(src) = sanitize_html_input(html) else {
        return Err(Error::new(
            error_class(),
            "out of memory decoding fragment HTML",
        ));
    };

    let root = context.parse(&parser, src.ptr, src.len);
    drop(src); /* the parse consumed it; the buffer goes on every path */
    drop(parser); /* the fragment belongs to its document, not to the parser */
    RawNode::from_ptr(root.cast())
        .ok_or_else(|| Error::new(error_class(), "failed to parse HTML fragment"))
}

/// Copy `src` into `doc`, `<template>` contents included, or `None` on failure.
///
/// The DOM `importNode` omits a template's separate content fragment, so every
/// copy in this extension is import-plus-fixup; this is that one operation.
/// Three callers wanted it with different deep flags, different error channels
/// and different messages, and each had grown its own copy of the four lines -
/// so the operation lives here and they keep only the parts that differ.
pub unsafe fn import_with_fixup(doc: RawDoc, src: RawNode, deep: bool) -> Option<RawNode> {
    import_raw(doc, src.as_ptr() as *mut LxbNode, deep)
        .and_then(|p| RawNode::from_ptr(p.cast()))
}

/// The raw form of [`import_with_fixup`]: `src` is a Lexbor node pointer, which
/// only this module uses, for the children it walks itself.
unsafe fn import_raw(doc: RawDoc, src: *mut LxbNode, deep: bool) -> Option<*mut LxbNode> {
    /* SAFETY: a live document and the caller's live source node. The handles
     * do not outlive this call. */
    let (Some(hdoc), Some(hsrc)) = (
        HtmlDoc::from_raw(doc.as_ptr() as *mut LxbDoc),
        HtmlNode::from_raw(src),
    ) else {
        return None;
    };
    let himp = hdoc.import_node(hsrc, deep)?;
    let imp = himp.as_raw();
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
pub unsafe fn html_import_deep(doc: RawDoc, src: RawNode) -> Result<RawNode, Error> {
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
    doc: RawDoc,
    context: Option<Value>,
) -> Result<(usize, usize), magnus::Error> {
    let Some(context) = context else {
        return Ok((TAG_BODY, NS_HTML));
    };
    if context.is_nil() {
        return Ok((TAG_BODY, NS_HTML));
    }

    if is_kind_of(context, &CLASS_NODE) {
        /* Reject an XML node before any Lexbor use. */
        let cn = html_node_unwrap(context)?.as_node();
        if cn.node_type() != LXB_DOM_NODE_TYPE_ELEMENT {
            return Err(magnus::Error::new(
                magnus::Ruby::get_unchecked().exception_arg_error(),
                "fragment context node must be an element",
            ));
        }
        return Ok((cn.tag_id(), cn.ns_id()));
    }

    /* A context tag name is a programmatic control string, not parsed HTML, so
     * it follows the strict text-input contract (valid UTF-8, no NUL). */
    let cv = ruby_verified_text(context, c"fragment context element")?;
    let name = cv.bytes();
    if name == b"svg" {
        return Ok((TAG_SVG, NS_SVG));
    }
    if name == b"math" {
        return Ok((TAG_MATH, NS_MATH));
    }
    let tid = lxb_tag_id_by_name_noi(
        (*(doc.as_ptr() as *mut LxbDoc)).tags,
        name.as_ptr(),
        name.len(),
    );
    if tid == TAG_UNDEF {
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
    doc: RawDoc,
    rb_html: Value,
    tag: usize,
    ns: usize,
) -> Result<Value, Error> {
    let html = ruby.into_value(rb_html.to_r_string()?);

    /* SAFETY: a live document, for the length of this call. */
    let frag = HtmlDoc::from_raw(doc.as_ptr() as *mut LxbDoc).and_then(HtmlDoc::create_fragment);
    let Some(frag) = frag else {
        return Err(Error::new(
            error_class(),
            "failed to create document fragment",
        ));
    };
    let frag_node = RawNode::from(frag);

    let root = run_fragment_parser(html.as_raw(), &FragmentContext::Tag { doc, tag, ns })?;
    if !import_fragment_children(doc, root, &Emit::Append(frag_node)) {
        return Err(Error::new(
            error_class(),
            "failed to import a fragment child",
        ));
    }
    Ok(wrap_html_node(frag_node, document))
}

/// The `context:` keyword, or None.
pub fn context_kwarg(ruby: &Ruby, kw: Option<magnus::RHash>) -> Option<Value> {
    let h = kw?;
    h.get(ruby.to_symbol("context"))
}
