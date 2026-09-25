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

use crate::lexbor::adapter::html::{self as dom, HtmlAttr, HtmlDoc, HtmlNode, RawNode};
use crate::lexbor::adapter::post_parse::HtmlParsed;
use crate::token::{Kind, Token};
use crate::xpath::abi::*;
use crate::xpath::ctx::Context;
use crate::xpath::dom::*;
use crate::xpath::msg::{Error, Status};

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
    /// # Safety
    /// `parsed` must be the live handle that owns `doc`, and stay live for
    /// `'d`, with nothing editing its document while `'d` lasts - which
    /// [`parsed`](Self::parsed) relies on to read it, and
    /// `HtmlParsed::tag_bucket` to lend its nodes for `'d`.
    unsafe fn new(doc: HtmlDoc<'d>, parsed: *mut HtmlParsed) -> HtmlDom<'d> {
        HtmlDom { doc, parsed }
    }

    /// The parsed handle, shared, for the evaluation's `'d`. Everything an
    /// evaluation reads goes through this; the one `&mut` is `prepare`'s.
    fn parsed(&self) -> &'d HtmlParsed {
        // SAFETY: `new`'s contract - live and unedited for `'d`.
        unsafe { &*self.parsed }
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
    #[allow(
        clippy::expect_used,
        reason = "an HTML token is minted only by `Token::html` from a live node"
    )]
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
    fn node_type(self, n: HtmlNode<'d>) -> NodeType {
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
        /* Rebuild the index a mutation dropped, so `//tag` is served from it;
         * an allocation failure fails the evaluate closed.
         *
         * This is the only `&mut` of the handle an evaluation takes, and it is
         * taken only when the index is missing - before this evaluation has
         * lent anything from it. A nested (handler-called) evaluate finds the
         * index its caller built and takes none, so no `&mut` ever overlaps a
         * bucket lent by `name_bucket`. */
        if self.parsed().dom_index().is_some() {
            return true;
        }
        // SAFETY: `new`'s contract, and nothing borrowed from the handle is
        // live: this evaluation has not started, and an outer one would have
        // built the index already.
        unsafe { (*self.parsed).ensure_dom_index() }
    }

    /// Served only for a document with no foreign element, where lax and
    /// strict admit the same elements.
    fn name_bucket(self, local: &[u8], ns_uri: Option<&[u8]>) -> Option<Bucket<'d, HtmlNode<'d>>> {
        let parsed = self.parsed();
        let index = parsed.dom_index()?;
        if ns_uri.is_some() || index.has_foreign() {
            return None;
        }
        let tag = self.doc.tag_id(local);
        if tag == dom::TAG_UNDEF || tag >= dom::TAG_LAST_ENTRY {
            return None;
        }
        /* Built by `prepare`; the handle lends the nodes for `'d`. */
        let nodes = parsed.tag_bucket(tag)?;
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
    Error::with(Status::Runtime, format_args!("evaluate with no document"))
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
    if !parsed.ensure_dom_index() {
        return Err(Error::with(
            Status::Oom,
            format_args!("out of memory building the element index"),
        ));
    }
    let parsed: *mut HtmlParsed = parsed;
    // SAFETY: this function's contract is `new`'s: `parsed` owns `doc` and is
    // live and unedited for `'e`.
    Ok(Context::new(
        unsafe { HtmlDom::new(doc, parsed) },
        Some(node),
    ))
}
