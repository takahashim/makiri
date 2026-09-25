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

use magnus::{prelude::*, Error, RString, Value};

use crate::bridge::ruby::makiri_error;

use crate::bridge::html::{html_node_unwrap, wrap_html_node};
use crate::bridge::ruby::{is_kind_of, string_of};
use crate::bridge::string::{ruby_verified_text, HtmlSource};
use crate::bridge::wrapper::{ensure_document_mutable, html_doc_unwrap, DocKind, DocumentShell};
use crate::init::CLASS_NODE;
use crate::lexbor::adapter::html::{
    HtmlDoc, HtmlNode, HtmlNodeMut, Place, RawDoc, RawNode, NS_HTML, NS_MATH, NS_SVG, TAG_BODY,
    TAG_MATH, TAG_SVG, TAG_UNDEF, TYPE_ELEMENT,
};
use crate::lexbor::adapter::post_parse::parse_html;
use crate::lexbor::fragment::{tag_id_by_name, FragmentContext, FragmentError, TransientFragment};

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
            return Err(crate::bridge::ruby::arg_error(
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
        return Err(crate::bridge::ruby::arg_error(format!(
            "unknown fragment context element: {}",
            String::from_utf8_lossy(name)
        )));
    }
    Ok(FragmentTag { tag, ns: NS_HTML })
}

/// Parse `html` as a fragment in `context`. Nothing is changed yet: a String
/// that fails to parse leaves every document as it was.
///
/// `html` is a String already - the caller ran `string_of` - because that
/// conversion is the argument's `#to_s`, arbitrary Ruby, and for `inner_html=`
/// it has to finish before the edit drops the document's indexes.
fn parse(html: RString, context: &FragmentContext) -> Result<TransientFragment, Error> {
    let src = HtmlSource::from_ruby(html.as_value())?;
    // SAFETY: the context's element or document is live (the callers below hold
    // it), and the bytes are read by the parse alone, which runs no Ruby.
    unsafe { TransientFragment::parse(src.bytes(), src.known_valid(), context) }
        .map_err(fragment_error)
}

/// Parse `rb_html` in the context of the element `context`, for `inner_html=`
/// and `outer_html=`, and import the result into a fresh DOCUMENT_FRAGMENT of
/// `context`'s document - detached, so the tree is still as it was.
///
/// That is what makes those two all-or-nothing. The import copies node by node
/// and can fail part-way; into the tree, that left the old content gone and the
/// new half in. Staged, a failure changes nothing a reader can see (the
/// abandoned fragment stays in the arena, which reclaims it with the document),
/// and what comes back is put in place with `HtmlNodeMut::place`, which only
/// relinks and cannot fail.
pub fn stage_fragment_in<'d>(
    context: HtmlNodeMut<'d>,
    html: RString,
) -> Result<HtmlNodeMut<'d>, Error> {
    let doc = context.node().owner_document();
    let Some(staged) = new_staged_fragment(doc) else {
        return Err(makiri_error("failed to create document fragment"));
    };
    import_into(context, staged, html)?;
    Ok(staged)
}

/// A fresh, detached DOCUMENT_FRAGMENT of `doc`, cleared for editing - the
/// staging target both fragment setters fill and place.
fn new_staged_fragment<'d>(doc: HtmlDoc<'d>) -> Option<HtmlNodeMut<'d>> {
    let staged = doc.create_fragment().map(RawNode::from)?;
    // SAFETY: just made in `doc`, detached, and nothing else refers to it.
    Some(unsafe { HtmlNodeMut::assume_mutable(staged.as_node()) })
}

/// Parse `html` in the context of the element `context` and import the result
/// into the DETACHED fragment `into`. Nothing in `into` is cleared first: on a
/// failure it is left as it was (a parse failure) or partly filled (an import
/// failure), and the caller only places it once this returned `Ok`.
fn import_into<'d>(
    context: HtmlNodeMut<'d>,
    into: HtmlNodeMut<'d>,
    html: RString,
) -> Result<(), Error> {
    let parsed = parse(
        html,
        &FragmentContext::Element(RawNode::from(context.node())),
    )?;
    let doc = context.node().owner_document();
    // SAFETY: `into` is a detached fragment of `context`'s document, which the
    // caller cleared for editing.
    if !unsafe { parsed.import_into(RawDoc::from(doc), RawNode::from(into.node())) } {
        return Err(makiri_error("failed to import a fragment child"));
    }
    Ok(())
}

/// The WHATWG `template.innerHTML = html`: parse `html` in the TEMPLATE
/// element's context and replace the children of its contents fragment, which
/// is where the specification puts a template's inner HTML (browsers read and
/// write `template.innerHTML` there, while the content fragment is what
/// `Element#content_fragment` exposes and `#to_html` serializes).
///
/// All or nothing, as [`stage_fragment_in`]: the contents change only after the
/// parse has succeeded.
pub fn set_template_inner_html(
    template: HtmlNodeMut<'_>,
    content: HtmlNodeMut<'_>,
    html: RString,
) -> Result<(), Error> {
    let doc = template.node().owner_document();
    let Some(staged) = new_staged_fragment(doc) else {
        return Err(makiri_error("failed to create document fragment"));
    };
    import_into(template, staged, html)?;
    while let Some(c) = content.first_child() {
        c.detach();
    }
    content.place(staged, Place::Child);
    Ok(())
}

/// A DOCUMENT_FRAGMENT owned by `document`, holding `rb_html` parsed in the
/// context `at`. The fragment node is made only once the parse has succeeded.
pub fn build_fragment(document: Value, rb_html: Value, at: FragmentTag) -> Result<Value, Error> {
    /* A fragment's nodes are made in `document`: a change to it, refused while
     * an XPath evaluation with a handler reads it. */
    ensure_document_mutable(document)?;
    /* `to_str`/`to_s` is Ruby code that may raise: converted under protect. */
    let html = string_of(rb_html)?;
    let doc = html_doc_unwrap(document)?;
    let parsed = parse(
        html,
        &FragmentContext::Tag {
            doc,
            tag: at.tag,
            ns: at.ns,
        },
    )?;

    let Some(frag) = crate::bridge::wrapper::html_doc(&document).create_fragment() else {
        return Err(makiri_error("failed to create document fragment"));
    };
    let frag = RawNode::from(frag);
    // SAFETY: `frag` was just made in `doc`, which nothing else is editing.
    if !unsafe { parsed.import_into(doc, frag) } {
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
