//! HTML fragments: parsing a String in a context, and splicing the result.
//!
//! One pipeline for every entry - `Document#fragment`, `DocumentFragment.parse`,
//! `Node#parse`, `inner_html=` and `outer_html=`. The input goes through
//! [`HtmlSource`] (a String, its encoding honoured), the parse
//! into a [`TransientFragment`] that owns a throwaway arena document, and
//! nothing in the target document changes until the parse has succeeded. The
//! fragment parser and the import primitives are [`crate::html_dom::fragment`]'s;
//! reading the Ruby arguments is the glue's.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;
use crate::bridge::rb::{prelude::*, Error, Value};

use crate::bridge::ruby::makiri_error;

use crate::bridge::html::{html_node_key, with_html_node, wrap_html_node};
use crate::bridge::ruby::{is_kind_of, string_arg};
use crate::bridge::string::{ruby_verified_text, HtmlSource};
use crate::bridge::wrapper::{doc_known, html_doc_check, html_read, new_html_document, DocHandle};
pub use crate::html_dom::fragment::FragmentTag;
use crate::html_dom::fragment::{FragmentContext, FragmentError, TransientFragment};
use crate::html_dom::html::{HtmlDoc, HtmlDocMut, HtmlNode, HtmlNodeKey, Place, RawNode};
use crate::html_dom::post_parse::parse_html;
pub use crate::html_dom::tree_guard::ParseLimits;
use crate::init::CLASS_NODE;

/// A fragment-parse failure as `Makiri::Error`.
fn fragment_error(e: FragmentError, limit: ParseLimits) -> Error {
    match e {
        FragmentError::Guard(stop) => crate::bridge::doc::guard_error(stop, limit),
        _ => makiri_error(e.message()),
    }
}

/// What a fragment is parsed inside of, as the caller gave it. An element is
/// read when the parse starts ([`build_fragment`]), after the HTML argument's
/// conversion.
pub enum ContextArg {
    /// A named context (nil is `<body>`).
    Tag(FragmentTag),
    /// An HTML element, read whole: the HTML Standard's fragment parsing takes
    /// its document's quirks mode, its ancestors' form and its attributes, as
    /// `inner_html=` does. Made by [`ContextArg::element`].
    Element(ContextElement),
}

/// The element of a [`ContextArg::Element`], already checked: its key and the
/// Document that keeps it alive. Taken out of its wrapper before the HTML
/// argument is converted, since that check can raise and the conversion's copy
/// must not be held across a raise; the element itself is read after.
pub struct ContextElement {
    key: HtmlNodeKey,
    document: Value,
}

impl ContextArg {
    /// The element wrapped by `v` as a context; `Err` for an XML node or a
    /// node that is not an element.
    pub fn element(ruby: &Ruby, v: Value) -> Result<ContextArg, Error> {
        let (key, document) = html_node_key(ruby, v)?;
        if !with_html_node(document, key, is_context_element)? {
            return Err(not_an_element());
        }
        Ok(ContextArg::Element(ContextElement { key, document }))
    }
}

/// Resolve the Ruby `context:` argument against `document`.
///
/// Nokogiri's `context:`: nil is `<body>` in the HTML namespace; a String names
/// an HTML-namespace tag, except "svg" / "math" which name the foreign roots;
/// an element is the context itself (the only way to reach a foreign non-root
/// context such as SVG `<desc>`), and may belong to another document -
/// `DocumentFragment.parse`'s always does. `Err` for an unusable one.
pub fn resolve_fragment_context(
    ruby: &Ruby,
    document: Value,
    context: Option<Value>,
) -> Result<ContextArg, Error> {
    let Some(context) = context.filter(|c| !c.is_nil()) else {
        return Ok(ContextArg::Tag(FragmentTag::BODY));
    };

    if is_kind_of(ruby, context, &CLASS_NODE) {
        /* Refused now, before the HTML argument is converted: an XML node or
         * a non-element. */
        return ContextArg::element(ruby, context);
    }

    /* A context tag name is a programmatic control string, not parsed HTML, so
     * it follows the strict text-input contract (valid UTF-8, no NUL). */
    let cv = ruby_verified_text(ruby, context, "fragment context element")?;
    let name = cv.as_bytes();
    /* The name table also holds the special ids it gives non-elements
     * (`#text`, `#document`, `!--`, ...), which would make the context a
     * node of that kind: only a name an element can have is a context. */
    if !crate::xml::dom_name::valid_element_local_name(name) {
        return Err(unknown_context(name));
    }
    if name == b"svg" {
        return Ok(ContextArg::Tag(FragmentTag::SVG));
    }
    if name == b"math" {
        return Ok(ContextArg::Tag(FragmentTag::MATH));
    }
    html_doc_check(document)?;
    let Some(tag) = html_read(&doc_known(document))?.doc().tag_id(name) else {
        return Err(unknown_context(name));
    };
    Ok(ContextArg::Tag(FragmentTag::html(tag)))
}

fn is_context_element(node: HtmlNode<'_>) -> bool {
    node.element().is_some() && node.tag_id().is_some()
}

fn not_an_element() -> Error {
    crate::bridge::ruby::arg_error("fragment context node must be an element")
}

fn unknown_context(name: &[u8]) -> Error {
    crate::bridge::ruby::arg_error(format_args!(
        "unknown fragment context element: {}",
        crate::msg::Lossy(name)
    ))
}

/// The context the element `node` provides to a fragment made in `target`.
fn element_context(node: HtmlNode<'_>, target: HtmlDoc<'_>) -> Result<FragmentContext, Error> {
    if !is_context_element(node) {
        return Err(not_an_element());
    }
    FragmentContext::element_into(node, target).map_err(|e| makiri_error(e.message()))
}

/// Parse `html` as a fragment in `context`. Nothing is changed yet: a String
/// that fails to parse leaves every document as it was.
///
/// `src` is converted already - the caller ran `HtmlSource::from_ruby` -
/// because that is the argument's transcode (a Ruby allocation), and for
/// `inner_html=` it has to finish before the edit drops the document's
/// indexes: nothing inside an edit may run Ruby.
fn parse(
    src: &HtmlSource,
    context: &FragmentContext,
    limit: ParseLimits,
) -> Result<TransientFragment, Error> {
    TransientFragment::parse(src.bytes(), src.known_valid(), context, limit)
        .map_err(|e| fragment_error(e, limit))
}

/// Parse `html` in the context of the element `context` (WHATWG fragment
/// parsing, as `inner_html=` and `outer_html=` do), and import the result into
/// a fresh DOCUMENT_FRAGMENT of the edited document - detached, so the tree is
/// still as it was.
///
/// That is what makes those two all-or-nothing. The import copies node by node
/// and can fail part-way; into the tree, that left the old content gone and the
/// new half in. Staged, a failure changes nothing a reader can see (the
/// abandoned fragment stays in the arena, which reclaims it with the document),
/// and what comes back is put in place with `HtmlDocMut::place`, which only
/// relinks and cannot fail.
pub fn stage_fragment_in(
    d: &mut HtmlDocMut<'_>,
    context: RawNode,
    src: &HtmlSource,
) -> Result<RawNode, Error> {
    let Some(staged) = d.create_fragment() else {
        return Err(makiri_error("failed to create document fragment"));
    };
    import_into(d, context, staged, src)?;
    Ok(staged)
}

/// Parse `html` in the context of the element `context` and import the result
/// into the DETACHED fragment `into`. Nothing in `into` is cleared first: on a
/// failure it is left as it was (a parse failure) or partly filled (an import
/// failure), and the caller only places it once this returned `Ok`.
fn import_into(
    d: &mut HtmlDocMut<'_>,
    context: RawNode,
    into: RawNode,
    src: &HtmlSource,
) -> Result<(), Error> {
    /* The setters take no keyword, so they parse under the default limit, as
     * Nokogiri's do. */
    let context = FragmentContext::element(d.read(), context)
        .map_err(|e| fragment_error(e, ParseLimits::DEFAULT))?;
    let parsed = parse(src, &context, ParseLimits::DEFAULT)?;
    parsed
        .import_into_doc(&mut d.new_nodes(), into)
        .map_err(|_| makiri_error("failed to import a fragment child"))
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
    d: &mut HtmlDocMut<'_>,
    template: RawNode,
    content: RawNode,
    src: &HtmlSource,
) -> Result<(), Error> {
    let staged = stage_fragment_in(d, template, src)?;
    d.detach_children(content);
    d.place(content, staged, Place::Child);
    Ok(())
}

/// A DOCUMENT_FRAGMENT owned by `document`, holding `rb_html` parsed in the
/// context `at`, refusing a tree deeper than `limit`. The fragment node is made
/// only once the parse has succeeded.
pub fn build_fragment(
    ruby: &Ruby,
    document: Value,
    rb_html: Value,
    at: ContextArg,
    limit: ParseLimits,
) -> Result<Value, Error> {
    /* A String only, transcoded to UTF-8 (a Ruby allocation, which may raise). */
    let src = HtmlSource::from_ruby(ruby, string_arg(rb_html)?)?;
    /* The element is read now, after the conversion, as `inner_html=` reads
     * it inside its edit. */
    let context = match at {
        ContextArg::Tag(at) => FragmentContext::tag(at),
        ContextArg::Element(el) => {
            html_doc_check(document)?;
            let handle = doc_known(document);
            let doc = html_read(&handle)?;
            with_html_node(el.document, el.key, |cn| element_context(cn, doc.doc()))??
        }
    };
    let parsed = parse(&src, &context, limit)?;

    let frag = crate::bridge::html::new_node(ruby, &document, |d| {
        let Some(frag) = d.create_fragment() else {
            return Err(makiri_error("failed to create document fragment"));
        };
        parsed
            .import_into_doc(d, frag)
            .map_err(|_| makiri_error("failed to import a fragment child"))?;
        Ok(frag)
    })??;
    /* Released before the wrap, which can raise (NoMemoryError) past this
     * frame: the source copy would leak with it, and so would the context's
     * namespace URI (`FragNs::Uri`, a context node of another document). */
    drop(src);
    drop(context);
    wrap_html_node(ruby, frag, document)
}

/// The standalone fragment's backing document: a throwaway
/// `<html><body></body></html>` shell, owned by the fragment's wrapper once
/// that is made; until then, by the handle returned here.
pub fn fragment_shell_document(ruby: &Ruby) -> Result<DocHandle, Error> {
    const SHELL: &[u8] = b"<html><body></body></html>";
    new_html_document(ruby, || {
        parse_html(SHELL, true, ParseLimits::DEFAULT)
            .map_err(|_| makiri_error("failed to create fragment document"))
    })
}
