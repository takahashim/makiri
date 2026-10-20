//! The XPath engine's HTML backend over the arena DOM (`crate::html::dom`):
//! the Lexbor backend (`lexbor::xpath`), read for read, over the arena's
//! handles. A token is the node's id (`Token::arena`), resolved by a checked
//! lookup, so nothing here needs `unsafe`.

use super::{HtmlAttr, HtmlDoc, HtmlNode, HtmlParsed, NodeId, NsId};
use crate::engine_error::ErrorKind;
use crate::token::{Kind, Token};
use crate::xpath::ctx::Context;
use crate::xpath::dom::*;

/// The HTML backend as an evaluate holds it: the parsed handle, whose
/// element index the bridge has built.
#[derive(Clone, Copy)]
pub struct HtmlDom<'d> {
    doc: HtmlDoc<'d>,
    parsed: &'d HtmlParsed,
}

/// The document behind `parsed`, as the backend reads it.
pub fn dom(parsed: &HtmlParsed) -> HtmlDom<'_> {
    HtmlDom {
        doc: parsed.doc(),
        parsed,
    }
}

/// A context over `parsed` with `node` as the focus, its element index built
/// first.
pub fn context(parsed: &HtmlParsed, node: Token) -> Context<'_, HtmlDom<'_>> {
    /* The walk serves a document the index cannot be built for. */
    let _ = parsed.ensure_dom_index();
    Context::new(dom(parsed), Some(node))
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
        Token::arena(super::RawNode::from(n).word())
    }
    #[inline]
    #[allow(
        clippy::expect_used,
        reason = "an arena token is minted only by `Dom::token` from a node of the document"
    )]
    fn resolve_token(self, t: Token) -> HtmlNode<'d> {
        assert_eq!(
            t.kind(),
            Kind::Arena,
            "an HTML context resolved a token of another kind"
        );
        let raw = super::RawNode::from_word(t.word()).expect("a resolved token names a node");
        self.doc
            .checked(raw)
            .expect("a token names a node of its document")
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
        lax || is_attr || ns.is_none() || ns == Some(NsId::HTML)
    }

    /// Lexbor gives an attribute with no namespace of its own its element's,
    /// so the attribute's own one is read (`HtmlAttr::own_ns_uri`).
    #[inline]
    fn attr_ns_uri(self, a: HtmlAttr<'d>) -> &'d [u8] {
        a.own_ns_uri().unwrap_or(&[])
    }

    #[inline]
    fn folds_name_case(self, el: HtmlNode<'d>) -> bool {
        el.ns_id() == Some(NsId::HTML)
    }
    #[inline]
    fn has_ns(self, n: HtmlNode<'d>) -> bool {
        n.ns_id().is_some()
    }
    #[inline]
    fn own_text(self, n: HtmlNode<'d>) -> &'d [u8] {
        n.own_text().unwrap_or(&[])
    }

    /// The element index is built by the evaluate's caller (the bridge's
    /// `ensure_dom_index`), so there is nothing to prepare: a document it
    /// could not be built for is walked.
    fn prepare(&self) -> Result<(), ErrorKind> {
        Ok(())
    }

    /// Served only for a document with no foreign element, where lax and
    /// strict admit the same elements, and no HTML element named with upper
    /// case as written, which no bucket holds but a folded name test finds.
    type BucketItem = NodeId;
    #[inline]
    #[allow(
        clippy::expect_used,
        reason = "a bucket holds node ids of this very document"
    )]
    fn bucket_node(self, item: NodeId) -> HtmlNode<'d> {
        self.doc
            .get(item)
            .expect("an indexed node of this document")
    }

    fn name_bucket(self, local: &[u8], ns_uri: Option<&[u8]>) -> Option<Bucket<'d, NodeId>> {
        let parsed = self.parsed;
        let index = parsed.dom_index()?;
        if ns_uri.is_some() || index.has_foreign() || index.has_case_kept() {
            return None;
        }
        let tag = self.doc.tag_id(local)?;
        /* None for a tag the index does not bucket, which falls back to the
         * walk. */
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
        if x.node().ns_id() != Some(NsId::XMLNS) {
            return Some(x);
        }
        a = x.next_attr();
    }
    None
}
