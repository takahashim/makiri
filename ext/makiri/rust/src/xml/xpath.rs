//! The XML binding of the node-access contract: `Dom` for `&xml::Document`.
//!
//! A node handle is an opaque, stamped [`xml::NodeId`]. It is not a slot index:
//! the stamp identifies the owning [`xml::Document`], and the low bits identify
//! the arena slot. The adapter never indexes the arena directly; every access
//! resolves through `Document`'s checked API.
//!
//! A handle coming from a node-set token is untrusted. `try_node` therefore
//! rejects a token naming another document, an invalid or out-of-range slot,
//! and any stale handle. The operation then reports no node, empty bytes, or
//! `false`, never a panic or a cross-document access.

#![forbid(unsafe_code)]

use crate::token::{Kind, Token};
use crate::xml::model as xml;
use crate::xpath::abi::*;
use crate::xpath::ctx::Context;
use crate::xpath::dom::{Bucket, Dom};

/// A namespace declaration is a NAMESPACE node in XPath 1.0, not an attribute,
/// so it must not appear on the attribute axis. The reader still keeps it as a
/// DOM attribute (Node#attribute_nodes reads `attrs` directly, matching DOM
/// Level 2); only the XPath iteration below skips it.
fn is_ns_decl(doc: &xml::Document, a: xml::NodeId) -> bool {
    let q = doc.try_node(a).map_or(&[][..], |x| doc.span(x.qname));
    /* `qname::xmlns_prefix` is the ONE definition of the shape; spelling the
     * prefix test out again here would be a third copy of it. */
    crate::xml::qname::xmlns_prefix(q).is_some()
}

fn skip_ns_decls(doc: &xml::Document, mut a: xml::NodeId) -> Option<xml::NodeId> {
    while is_ns_decl(doc, a) {
        a = doc.next(a)?;
    }
    Some(a)
}

impl<'d> Dom<'d> for &'d xml::Document {
    /* ---- host policy: XML's (see `Dom`) ---- */
    const ID_ATTRIBUTE: Option<&'static [u8]> = None;
    const LANG_ATTRIBUTES: &'static [&'static [u8]] = &[b"xml:lang"];

    type Node = xml::NodeId;
    /// Checked once, by `as_attr` or by coming out of `first_attr`.
    type Attr = xml::NodeId;

    #[inline]
    fn token(n: xml::NodeId) -> Token {
        Token::xml(n.to_token())
    }
    /// The token is opaque data: every read resolves it through
    /// `Document::try_node`, so a stale or foreign one reads as no node.
    #[inline]
    fn resolve_token(self, t: Token) -> xml::NodeId {
        /* An HTML token never reaches an XML context; assert it here so a bug
         * shows as a check, not a silently misread arena slot. */
        assert_eq!(
            t.kind(),
            Kind::Xml,
            "an XML context resolved a non-XML token"
        );
        xml::NodeId::from_token(t.word())
    }

    #[inline]
    fn document_node(self) -> xml::NodeId {
        self.doc_node()
    }
    #[inline]
    fn node_type(self, n: xml::NodeId) -> u32 {
        self.try_node(n).map_or(0, |x| x.type_.as_u32())
    }

    #[inline]
    fn first_child(self, n: xml::NodeId) -> Option<xml::NodeId> {
        xml::Document::first_child(self, n)
    }
    #[inline]
    fn last_child(self, n: xml::NodeId) -> Option<xml::NodeId> {
        xml::Document::last_child(self, n)
    }
    #[inline]
    fn next(self, n: xml::NodeId) -> Option<xml::NodeId> {
        xml::Document::next(self, n)
    }
    #[inline]
    fn prev(self, n: xml::NodeId) -> Option<xml::NodeId> {
        xml::Document::prev(self, n)
    }
    #[inline]
    fn parent(self, n: xml::NodeId) -> Option<xml::NodeId> {
        xml::Document::parent(self, n)
    }

    #[inline]
    fn first_attr(self, el: xml::NodeId) -> Option<xml::NodeId> {
        skip_ns_decls(self, xml::Document::attrs(self, el)?)
    }
    #[inline]
    fn attr_next(self, a: xml::NodeId) -> Option<xml::NodeId> {
        skip_ns_decls(self, xml::Document::next(self, a)?)
    }
    #[inline]
    fn attr_node(a: xml::NodeId) -> xml::NodeId {
        a
    }
    #[inline]
    fn as_attr(self, n: xml::NodeId) -> Option<xml::NodeId> {
        self.try_node(n)
            .is_some_and(|x| x.type_.as_u32() == crate::xpath::dom::NTYPE_ATTRIBUTE)
            .then_some(n)
    }
    #[inline]
    fn attr_value(self, a: xml::NodeId) -> &'d [u8] {
        self.try_node(a).map_or(&[], |x| self.span(x.value))
    }
    fn get_attribute(self, el: xml::NodeId, name: &[u8]) -> Option<&'d [u8]> {
        let mut a = xml::Document::attrs(self, el);
        while let Some(id) = a {
            if let Some(x) = self.try_node(id) {
                if !is_ns_decl(self, id) && self.span(x.qname) == name {
                    return Some(self.span(x.value));
                }
            }
            a = xml::Document::next(self, id);
        }
        None
    }

    #[inline]
    fn local_name(self, n: xml::NodeId) -> &'d [u8] {
        self.try_node(n).map_or(&[], |x| self.span(x.local))
    }
    #[inline]
    fn attr_local_name(self, a: xml::NodeId) -> &'d [u8] {
        self.try_node(a).map_or(&[], |x| self.span(x.local))
    }
    #[inline]
    fn qualified_name(self, n: xml::NodeId) -> &'d [u8] {
        self.try_node(n).map_or(&[], |x| self.span(x.qname))
    }
    #[inline]
    fn attr_qualified_name(self, a: xml::NodeId) -> &'d [u8] {
        self.try_node(a).map_or(&[], |x| self.span(x.qname))
    }
    #[inline]
    fn pi_name(self, n: xml::NodeId) -> &'d [u8] {
        self.try_node(n).map_or(&[], |x| self.span(x.local))
    }

    #[inline]
    fn ns_uri(self, n: xml::NodeId) -> &'d [u8] {
        self.try_node(n).map_or(&[], |x| self.span(x.ns_uri))
    }

    /// A name test compares the local name, prefixed or not: the namespace
    /// rule decides the rest.
    #[inline]
    fn test_name(self, n: xml::NodeId, _prefixed: bool) -> &'d [u8] {
        Dom::local_name(self, n)
    }
    #[inline]
    fn attr_test_name(self, a: xml::NodeId, _prefixed: bool) -> &'d [u8] {
        Dom::attr_local_name(self, a)
    }

    /// An unprefixed test matches a node in no namespace only, element or
    /// attribute - lax included, since `Nokogiri::XML` (libxml2) is as strict.
    #[inline]
    fn unprefixed_matches(self, n: xml::NodeId, _is_attr: bool, _lax: bool) -> bool {
        self.try_node(n).is_some_and(|x| x.ns_uri.len == 0)
    }

    /// An attribute node carries its own namespace.
    #[inline]
    fn attr_ns_uri(self, a: xml::NodeId) -> &'d [u8] {
        Dom::ns_uri(self, a)
    }

    /// XML names are case-sensitive.
    #[inline]
    fn folds_name_case(self, _el: xml::NodeId) -> bool {
        false
    }

    #[inline]
    fn has_ns(self, n: xml::NodeId) -> bool {
        self.try_node(n).is_some_and(|x| x.ns_uri.len != 0)
    }

    /// The node owns its value, so this is an append of a borrowed slice.
    #[inline]
    fn append_own_text(self, n: xml::NodeId, buf: &mut Buf) -> Result<(), BufError> {
        let s = self.try_node(n).map_or(&[][..], |x| self.span(x.value));
        if s.is_empty() {
            return Ok(());
        }
        buf.append(s)
    }

    /// The XML name index is keyed by (local name, namespace URI), so a bucket
    /// holds exactly the matching elements and needs no re-check. An
    /// unprefixed test means no namespace, in either mode.
    fn name_bucket(self, local: &[u8], ns_uri: Option<&[u8]>) -> Option<Bucket<'d, xml::NodeId>> {
        let uri = ns_uri.unwrap_or(b"");
        /* Built lazily and cached on the document; None on OOM, and the caller
         * walks. */
        let idx = self.name_index()?;
        Some(Bucket {
            nodes: idx.lookup(local, uri),
            recheck: false,
        })
    }
}

/// A context over `doc` with `node` as the focus; the bridge passes the document
/// node for a whole-document query.
pub fn context(doc: &xml::Document, node: xml::NodeId) -> Context<'_, &xml::Document> {
    Context::new(doc, Token::xml(node.to_token()))
}
