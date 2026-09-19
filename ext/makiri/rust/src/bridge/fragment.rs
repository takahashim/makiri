//! The Ruby-facing half of fragment parsing: `Document#fragment(html,
//! context:)`, `DocumentFragment.parse`, and `Node#parse`.
//!
//! The Lexbor parser, the import primitives and the context tag lookup live in
//! [`crate::lexbor::fragment`]; this layer resolves the Ruby `context:` argument
//! (a node, a tag name, or nil) and wraps the fragment it builds.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use magnus::{prelude::*, Error, Ruby, Value};

use crate::bridge::html::{html_node_unwrap, wrap_html_node};
use crate::bridge::ruby::{error_class, is_kind_of};
use crate::bridge::string::ruby_verified_text;
use crate::bridge::string::HtmlSource;
use crate::init::CLASS_NODE;
use crate::lexbor::adapter::html::{
    HtmlDoc, RawDoc, RawNode, NS_HTML, NS_MATH, NS_SVG, TAG_BODY, TAG_MATH, TAG_SVG, TAG_UNDEF,
    TYPE_ELEMENT,
};
use crate::lexbor::fragment::{
    import_fragment_children, run_fragment_parser, tag_id_by_name, Emit, FragmentContext,
    FragmentError,
};

/// A fragment-parse failure as `Makiri::Error`.
pub fn fragment_error(e: FragmentError) -> Error {
    Error::new(error_class(), e.message())
}

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
) -> Result<(usize, usize), Error> {
    let Some(context) = context else {
        return Ok((TAG_BODY, NS_HTML));
    };
    if context.is_nil() {
        return Ok((TAG_BODY, NS_HTML));
    }

    if is_kind_of(context, &CLASS_NODE) {
        /* Reject an XML node before any Lexbor use. */
        let cn = html_node_unwrap(context)?.as_node();
        if cn.node_type() != TYPE_ELEMENT {
            return Err(Error::new(
                Ruby::get_unchecked().exception_arg_error(),
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
    let tid = tag_id_by_name(doc, name);
    if tid == TAG_UNDEF {
        /* The C wrote `"...: %" PRIsVALUE`; `%.*s` over the verified bytes
         * prints what PRIsVALUE printed for a String: its content. */
        return Err(Error::new(
            Ruby::get_unchecked().exception_arg_error(),
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
    let frag = HtmlDoc::from_raw(doc.as_ptr() as *mut _).and_then(HtmlDoc::create_fragment);
    let Some(frag) = frag else {
        return Err(Error::new(
            error_class(),
            "failed to create document fragment",
        ));
    };
    let frag_node = RawNode::from(frag);

    let src = HtmlSource::from_ruby(html)?;
    let root = run_fragment_parser(
        src.bytes(),
        src.known_valid(),
        &FragmentContext::Tag { doc, tag, ns },
    )
    .map_err(fragment_error)?;
    drop(src);
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
