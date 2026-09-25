//! The XPath engine's HTML backend: `Dom` for a Lexbor document, through
//! [`crate::lexbor::adapter::html`], which is the only module that reads Lexbor's
//! structs.
//!
//! The adapter's typed handles carry the contract - a live node of a document
//! that is not restructured while they are held - so the readers here are safe.
//! What they add is the kind check the raw readers leave to their callers: an
//! element-only or attribute-only read of another kind of node answers empty
//! rather than reading it as the wrong struct.

#![allow(unsafe_code)]

use crate::lexbor::abi as lxb;
use crate::lexbor::adapter::dom_index::DomIndex;
use crate::lexbor::adapter::html::{self as dom, HtmlAttr, HtmlDoc, HtmlNode, RawNode};
use crate::lexbor::adapter::post_parse::HtmlParsed;
use crate::token::{Kind, Token};
use crate::xpath::abi::*;
use crate::xpath::ctx::Context;
use crate::xpath::dom::*;
use crate::xpath::msg::{Error, XP_ERR_OOM, XP_ERR_RUNTIME};

/* The engine reads every node's type through the shared `NTYPE_*` encoding, so
 * Lexbor's enum must agree value for value; a mismatch would make an HTML walk
 * misread each node rather than fail. Checked at compile time, against the
 * generated header view, in every build that has an HTML backend. */
const _: () = {
    assert!(NTYPE_ELEMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT);
    assert!(NTYPE_ATTRIBUTE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ATTRIBUTE);
    assert!(NTYPE_TEXT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT);
    assert!(NTYPE_CDATA_SECTION == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION);
    assert!(NTYPE_ENTITY_REFERENCE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ENTITY_REFERENCE);
    assert!(NTYPE_ENTITY == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ENTITY);
    assert!(NTYPE_PI == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_PROCESSING_INSTRUCTION);
    assert!(NTYPE_COMMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_COMMENT);
    assert!(NTYPE_DOCUMENT == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT);
    assert!(NTYPE_DOCUMENT_TYPE == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_TYPE);
    assert!(NTYPE_NOTATION == lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_NOTATION);
};

/// The HTML backend as an evaluate holds it: the document, and the parsed handle
/// its element index is read from.
///
/// The handle, not the index: a mutation between evaluates drops the index, so
/// each evaluate reads it afresh ([`Dom::prepare`]) rather than keeping a stale
/// one.
#[derive(Clone, Copy)]
pub struct HtmlDom<'d> {
    doc: HtmlDoc<'d>,
    parsed: *mut HtmlParsed,
}

impl<'d> HtmlDom<'d> {
    /// `parsed` must be the live handle behind `doc`.
    pub fn new(doc: HtmlDoc<'d>, parsed: *mut HtmlParsed) -> HtmlDom<'d> {
        HtmlDom { doc, parsed }
    }

    /// The document's element index as it stands now, rebuilding it
    /// after a mutation. `None` on OOM.
    fn index(&self) -> Option<&DomIndex> {
        // SAFETY: the caller's contract - `parsed` is live for `'d`, and no
        // mutation runs while an evaluate on it does.
        unsafe { (*self.parsed).dom_index() }
    }
}

impl<'d> Dom<'d> for HtmlDom<'d> {
    /* ---- host policy: HTML's (see `Dom`) ---- */
    const ID_ATTRIBUTE: Option<&'static [u8]> = Some(b"id");
    /// HTML's own `lang` first, then XPath 1.0's `xml:lang`.
    const LANG_ATTRIBUTES: &'static [&'static [u8]] = &[b"lang", b"xml:lang"];

    type Node = HtmlNode<'d>;
    type Attr = HtmlAttr<'d>;

    #[inline]
    fn token(n: HtmlNode<'d>) -> Token {
        // SAFETY: `n` is a live node of this document, which `HtmlDom` holds.
        unsafe { Token::html(RawNode::from(n).as_ptr()) }
    }
    #[inline]
    fn resolve_token(self, t: Token) -> HtmlNode<'d> {
        /* The kind check is the safety gate: only `Token::html` (unsafe) makes
         * an HTML token, so a token that reaches here names a live node. */
        assert_eq!(
            t.kind(),
            Kind::Html,
            "an HTML context resolved a non-HTML token"
        );
        let raw = RawNode::from_ptr(t.as_ptr()).expect("a resolved token names a node");
        // SAFETY: an HTML token names a live node of the document it was made
        // over, and `self` is that document.
        unsafe { raw.as_node() }
    }

    #[inline]
    fn document_node(self) -> HtmlNode<'d> {
        self.doc.as_node()
    }
    #[inline]
    fn node_type(self, n: HtmlNode<'d>) -> u32 {
        n.node_type()
    }

    #[inline]
    fn first_child(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.first_child()
    }
    #[inline]
    fn last_child(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.last_child()
    }
    #[inline]
    fn next(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.next()
    }
    #[inline]
    fn prev(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.prev()
    }
    #[inline]
    fn parent(self, n: HtmlNode<'d>) -> Option<HtmlNode<'d>> {
        n.parent()
    }

    /* A namespace declaration is not an attribute in XPath's data model, so the
     * axis skips the ones the parser put in the XMLNS namespace - the XML
     * backend does the same. An `xmlns` on an HTML element is an ordinary
     * no-namespace attribute and stays. */
    #[inline]
    fn first_attr(self, el: HtmlNode<'d>) -> Option<HtmlAttr<'d>> {
        skip_ns_decls(el.element()?.first_attr())
    }
    #[inline]
    fn attr_next(self, a: HtmlAttr<'d>) -> Option<HtmlAttr<'d>> {
        skip_ns_decls(a.next_attr())
    }
    #[inline]
    fn attr_node(a: HtmlAttr<'d>) -> HtmlNode<'d> {
        a.node()
    }
    #[inline]
    fn as_attr(self, n: HtmlNode<'d>) -> Option<HtmlAttr<'d>> {
        n.attr()
    }
    #[inline]
    fn attr_value(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.value()
    }
    #[inline]
    fn get_attribute(self, el: HtmlNode<'d>, name: &[u8]) -> Option<&'d [u8]> {
        el.element()?.get_attribute(name)
    }

    #[inline]
    /// The DOM's `localName`, case preserved (`foreignObject`, not Lexbor's
    /// `foreignobject`).
    fn local_name(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.element().map_or(&[], |e| e.dom_local_name())
    }
    #[inline]
    /// The DOM's `localName`, case preserved (`refX`, not Lexbor's `refx`).
    fn attr_local_name(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.dom_local_name()
    }
    #[inline]
    fn qualified_name(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.qualified_name()
    }
    #[inline]
    fn attr_qualified_name(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.qualified_name()
    }
    #[inline]
    fn pi_name(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.node_name()
    }

    #[inline]
    fn ns_uri(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.ns_uri().unwrap_or(&[])
    }
    /// A prefixed test compares the local name; an unprefixed one the
    /// qualified name, as browsers do.
    #[inline]
    fn test_name(self, n: HtmlNode<'d>, prefixed: bool) -> &'d [u8] {
        if prefixed {
            Dom::local_name(self, n)
        } else {
            Dom::qualified_name(self, n)
        }
    }
    #[inline]
    fn attr_test_name(self, a: HtmlAttr<'d>, prefixed: bool) -> &'d [u8] {
        if prefixed {
            a.dom_local_name()
        } else {
            a.qualified_name()
        }
    }

    /// Strict: an unprefixed element test resolves in the HTML namespace (or
    /// none), so a foreign SVG / MathML element needs a prefix. Attributes are
    /// exempt: the qualified-name compare already set the prefixed ones apart.
    /// Lax is `Nokogiri::HTML`, which has no namespaces: anything goes.
    #[inline]
    fn unprefixed_matches(self, n: HtmlNode<'d>, is_attr: bool, lax: bool) -> bool {
        let ns = n.ns_id();
        lax || is_attr || ns == dom::NS_HTML || ns == dom::NS_UNDEF
    }

    /// Lexbor gives an attribute with no namespace of its own its element's,
    /// so the attribute's own one is read (`HtmlAttr::own_ns_uri`).
    #[inline]
    fn attr_ns_uri(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.own_ns_uri().unwrap_or(&[])
    }

    #[inline]
    fn folds_name_case(self, el: HtmlNode<'d>) -> bool {
        el.ns_id() == dom::NS_HTML
    }
    #[inline]
    fn has_ns(self, n: HtmlNode<'d>) -> bool {
        n.ns_id() != dom::NS_UNDEF
    }
    #[inline]
    fn append_own_text(self, n: HtmlNode<'d>, buf: &mut Buf) -> Result<(), BufError> {
        n.with_text_content(|text| text.map_or(Ok(()), |t| buf.append(t)))
    }

    fn prepare(&self) -> bool {
        /* Reading the index builds it when a mutation dropped it, so `//tag`
         * is served from it; an allocation failure fails the evaluate closed. */
        self.index().is_some()
    }

    /// Served only for a document with no foreign element, where lax and
    /// strict admit the same elements.
    fn name_bucket(self, local: &[u8], ns_uri: Option<&[u8]>) -> Option<Bucket<'d, HtmlNode<'d>>> {
        let index = self.index()?;
        if ns_uri.is_some() || index.has_foreign() {
            return None;
        }
        let tag = self.doc.tag_id(local);
        if tag == dom::TAG_UNDEF || tag >= dom::TAG_LAST_ENTRY {
            return None;
        }
        let nodes = index.tag_bucket(tag);
        // SAFETY: the index holds only live elements of this document, and it
        // lives as long as the evaluation over `'d` (no mutation runs during
        // one, and a mutation is what drops the index).
        let nodes: &'d [HtmlNode<'d>] = unsafe { RawNode::as_nodes(nodes) };
        Some(Bucket {
            nodes,
            recheck: true,
        })
    }
}

/// `a`, or the first attribute after it that is not a namespace declaration.
fn skip_ns_decls(mut a: Option<HtmlAttr<'_>>) -> Option<HtmlAttr<'_>> {
    while let Some(x) = a {
        if x.node().ns_id() != dom::NS_XMLNS {
            return Some(x);
        }
        a = x.next_attr();
    }
    None
}

/* ------------------------------------------------------------------ */
/* the backend's context                                              */
/* ------------------------------------------------------------------ */

/// `evaluate with no document`.
fn no_document() -> Error {
    Error::with(XP_ERR_RUNTIME, format_args!("evaluate with no document"))
}

/// A context over the HTML document behind `parsed`, with its element/attribute
/// index as it stands now, and `node` as the focus.
///
/// The index serves `//tag`; an attribute's parent is Lexbor's own
/// `attr->owner` (see `HtmlNode::parent`). Each evaluate reads the index afresh
/// from the handle, so a mutation between evaluates drops it and the next
/// evaluate rebuilds it - the context must not keep the one it saw here.
///
/// # Safety
/// `parsed` must stay live and free of mutation for `'e`, and `node` must be a
/// node of its document.
#[allow(clippy::result_large_err)]
pub unsafe fn context<'e>(
    parsed: *mut HtmlParsed,
    node: Token,
) -> Result<Context<'e, HtmlDom<'e>>, Error> {
    // SAFETY: the caller's contract - the handle is live for `'e`.
    let Some(parsed) = (unsafe { parsed.as_mut() }) else {
        return Err(no_document());
    };
    // SAFETY: as above - the document the handle owns, live for `'e`.
    let doc: HtmlDoc<'e> = unsafe { parsed.raw_doc().as_doc() };
    /* Build it now, so an allocation failure is reported here rather than on
     * the first evaluate. Each evaluate still re-reads it through the handle. */
    if parsed.dom_index().is_none() {
        return Err(Error::with(
            XP_ERR_OOM,
            format_args!("out of memory building the element index"),
        ));
    }
    let parsed: *mut HtmlParsed = parsed;
    Ok(Context::new(HtmlDom::new(doc, parsed), node))
}
