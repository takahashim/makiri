//! HTML fragments: parsing a String in a context, and splicing the result.
//!
//! One pipeline for every entry - `Document#fragment`, `DocumentFragment.parse`,
//! `Node#parse`, `inner_html=` and `outer_html=`. The input goes through
//! [`HtmlSource`] (converted under protect, its encoding honoured), the parse
//! into a [`TransientFragment`] that owns Lexbor's throwaway document, and
//! nothing in the target document changes until the parse has succeeded. The
//! Lexbor parsers and the import primitives are [`crate::lexbor::fragment`]'s;
//! reading the Ruby arguments is the glue's.

#![allow(unsafe_code)]

use magnus::{prelude::*, Error, Ruby, Value};

use crate::bridge::ruby::makiri_error;

use crate::bridge::html::{html_node_unwrap, wrap_html_node};
use crate::bridge::ruby::{is_kind_of, string_of};
use crate::bridge::string::{ruby_verified_text, HtmlSource};
use crate::bridge::wrapper::{ensure_document_mutable, html_doc_unwrap, DocKind, DocumentShell};
use crate::init::CLASS_NODE;
use crate::lexbor::adapter::html::{
    HtmlDoc, HtmlNode, HtmlNodeMut, RawDoc, RawNode, NS_HTML, NS_MATH, NS_SVG, TAG_BODY, TAG_MATH,
    TAG_SVG, TAG_UNDEF, TYPE_ELEMENT,
};
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::fragment::{
    tag_id_by_name, Emit, FragmentContext, FragmentError, TransientFragment,
};

/// A fragment-parse failure as `Makiri::Error`.
fn fragment_error(e: FragmentError) -> Error {
    makiri_error(e.message())
}

/// The context a fragment is parsed "inside of", per the WHATWG algorithm, as
/// the tag and namespace ids the parser takes - a pair that used to travel as
/// two bare `usize`s, where swapping them compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentTag {
    pub tag: usize,
    pub ns: usize,
}

impl FragmentTag {
    /// `<body>` in the HTML namespace - the context when none is given.
    pub const BODY: FragmentTag = FragmentTag {
        tag: TAG_BODY,
        ns: NS_HTML,
    };

    /// The context an element provides: its own tag and namespace.
    pub fn of(el: HtmlNode<'_>) -> FragmentTag {
        FragmentTag {
            tag: el.tag_id(),
            ns: el.ns_id(),
        }
    }
}

/// Resolve the Ruby `context:` argument against `document`.
///
/// Matches Nokogiri's `context:`: nil is `<body>` in the HTML namespace; a node
/// contributes its own tag and namespace (the only way to reach a foreign
/// non-root context such as SVG `<desc>`); a String names an HTML-namespace tag,
/// except "svg" / "math" which name the foreign roots. `Err` for an unusable one.
pub fn resolve_fragment_context(
    document: Value,
    context: Option<Value>,
) -> Result<FragmentTag, Error> {
    let Some(context) = context.filter(|c| !c.is_nil()) else {
        return Ok(FragmentTag::BODY);
    };

    if is_kind_of(context, &CLASS_NODE) {
        /* Reject an XML node before any Lexbor use. */
        // SAFETY: `unwrap` checked it is an HTML node, which `context` keeps
        // alive for this call.
        let cn = unsafe { html_node_unwrap(context)?.as_node() };
        if cn.node_type() != TYPE_ELEMENT {
            return Err(Error::new(
                Ruby::get_with(context).exception_arg_error(),
                "fragment context node must be an element",
            ));
        }
        return Ok(FragmentTag::of(cn));
    }

    /* A context tag name is a programmatic control string, not parsed HTML, so
     * it follows the strict text-input contract (valid UTF-8, no NUL). */
    let cv = ruby_verified_text(context, c"fragment context element")?;
    let name = cv.as_verified().as_bytes();
    if name == b"svg" {
        return Ok(FragmentTag {
            tag: TAG_SVG,
            ns: NS_SVG,
        });
    }
    if name == b"math" {
        return Ok(FragmentTag {
            tag: TAG_MATH,
            ns: NS_MATH,
        });
    }
    let tag = tag_id_by_name(html_doc_unwrap(document)?, name);
    if tag == TAG_UNDEF {
        return Err(Error::new(
            Ruby::get_with(context).exception_arg_error(),
            format!(
                "unknown fragment context element: {}",
                String::from_utf8_lossy(name)
            ),
        ));
    }
    Ok(FragmentTag { tag, ns: NS_HTML })
}

/// Parse `rb_html` as a fragment in `context`. Nothing is changed yet: a String
/// that fails to convert or parse leaves every document as it was.
fn parse(rb_html: Value, context: &FragmentContext) -> Result<TransientFragment, Error> {
    /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
    let html = string_of(rb_html)?.as_value();
    let src = HtmlSource::from_ruby(html)?;
    // SAFETY: the context's element or document is live (the callers below hold
    // it), and the bytes are read by the parse alone, which runs no Ruby.
    unsafe { TransientFragment::parse(src.bytes(), src.known_valid(), context) }
        .map_err(fragment_error)
}

/// Parse `rb_html` in the context of the element `context`, for
/// `inner_html=` / `outer_html=`; splice it in with [`splice_fragment`].
pub fn parse_fragment_in(
    context: HtmlNode<'_>,
    rb_html: Value,
) -> Result<TransientFragment, Error> {
    parse(rb_html, &FragmentContext::Element(RawNode::from(context)))
}

/// Where [`splice_fragment`] puts the fragment's children.
#[derive(Clone, Copy)]
pub enum Place {
    /// As the last children of the node.
    Append,
    /// Just before the node, under its parent.
    Before,
}

/// Import `frag`'s children into `at`'s document, placed by `place`.
pub fn splice_fragment(
    frag: TransientFragment,
    at: HtmlNodeMut<'_>,
    place: Place,
) -> Result<(), Error> {
    let node = RawNode::from(at.node());
    let emit = match place {
        Place::Append => Emit::Append(node),
        Place::Before => Emit::Before(node),
    };
    // SAFETY: `at` is a live node the caller cleared for editing, and its
    // document is the one the children go into.
    if !unsafe { frag.import_into(RawDoc::from(at.node().owner_document()), &emit) } {
        return Err(makiri_error("failed to import a fragment child"));
    }
    Ok(())
}

/// A DOCUMENT_FRAGMENT owned by `document`, holding `rb_html` parsed in the
/// context `at`. The fragment node is made only once the parse has succeeded.
pub fn build_fragment(document: Value, rb_html: Value, at: FragmentTag) -> Result<Value, Error> {
    /* A fragment's nodes are made in `document`: a change to it, refused while
     * an XPath evaluation with a handler reads it. */
    ensure_document_mutable(document)?;
    let doc = html_doc_unwrap(document)?;
    let parsed = parse(
        rb_html,
        &FragmentContext::Tag {
            doc,
            tag: at.tag,
            ns: at.ns,
        },
    )?;

    // SAFETY: `document`'s live Lexbor document, for the length of this call.
    let frag =
        unsafe { HtmlDoc::from_raw(doc.as_ptr() as *mut _) }.and_then(HtmlDoc::create_fragment);
    let Some(frag) = frag else {
        return Err(makiri_error("failed to create document fragment"));
    };
    let frag = RawNode::from(frag);
    // SAFETY: `frag` was just made in `doc`, which nothing else is editing.
    if !unsafe { parsed.import_into(doc, &Emit::Append(frag)) } {
        return Err(makiri_error("failed to import a fragment child"));
    }
    Ok(wrap_html_node(frag, document))
}

/// The standalone fragment's backing document: a throwaway
/// `<html><body></body></html>` shell, owned by the fragment's wrapper.
pub fn fragment_shell_document() -> Result<Value, Error> {
    const SHELL: &[u8] = b"<html><body></body></html>";
    /* The wrapper first, while nothing needs freeing - see DocumentShell. */
    let shell = DocumentShell::new(DocKind::Html);
    let Some(parsed) = parse_html(SHELL, true) else {
        return Err(makiri_error("failed to create fragment document"));
    };
    Ok(shell.install_html(parsed))
}
