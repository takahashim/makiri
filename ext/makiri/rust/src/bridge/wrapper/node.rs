//! The node wrapper: the word a wrapper stores, its TypedData types, and
//! the accessors that read a node out of any wrapper.

#![forbid(unsafe_code)]

use crate::bridge::rb::Ruby;

use crate::bridge::rb::{Error, Value};

use super::*;
use crate::bridge::typed::EmbeddedType;
use crate::html_dom::html::{HtmlNodeKey, RawNode};
use crate::init::CLASS_DOCUMENT;
use crate::token::Token;
use crate::xml::model::NodeId;

/* ------------------------------------------------------------------ *
 * the node wrapper                                                   *
 * ------------------------------------------------------------------ */

/// A node as the Ruby layer stores it: one pointer-sized word that is an arena
/// node id - the HTML arena's in an HTML document, an XML [`NodeId`] in an
/// XML one.
///
/// Which of the two it is is not stored beside it. It is the document's
/// representation - the wrapper's TypedData type, a NodeSet's [`DocKind`] -
/// decided once, so a word is read back only through the reader named for
/// that kind ([`xml`](Self::xml), [`html`](Self::html), [`token`](Self::token)).
/// This type is where the word crosses between the typed forms and the
/// untyped one, and so the one place in the glue a stored node is cast.
///
/// `repr(transparent)` over `usize`: a NodeSet's buffer keeps the layout it had
/// as `*mut c_void`. (A [`NodeData`] does not use it any more - it stores an
/// opaque [`NodeHandle`], which is larger by design.)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(transparent)]
pub struct NodeWord(usize);

impl From<NodeId> for NodeWord {
    #[inline]
    fn from(id: NodeId) -> Self {
        NodeWord(id.to_token())
    }
}

impl From<RawNode> for NodeWord {
    #[inline]
    fn from(n: RawNode) -> Self {
        NodeWord(n.word())
    }
}

impl NodeWord {
    /// The word of no node: a placeholder in a buffer not yet written, never
    /// stored in a set.
    pub const NULL: NodeWord = NodeWord(0);

    /// The word of an engine token. The token's kind is dropped: the word goes
    /// into a set or wrapper whose document already names it.
    #[inline]
    pub fn of_token(t: Token) -> Self {
        NodeWord(t.word())
    }

    /// The XML node, or `None` for a word naming no slot (a null one) - the
    /// same shape as [`html`](Self::html). Safe for any word: an arena reads a
    /// `NodeId` through `Document::try_node`, which rejects a stale or foreign
    /// one.
    #[inline]
    pub fn xml(self) -> Option<NodeId> {
        NodeId::from_token(self.0)
    }

    /// The HTML node; `None` for a null word. Safe for any word, as
    /// [`xml`](Self::xml) is: the arena checks the id when it is resolved.
    #[inline]
    pub fn html(self) -> Option<RawNode> {
        RawNode::from_word(self.0)
    }

    /// The engine token for this node of a document of kind `kind`. Infallible:
    /// a [`DocKind`] is HTML or XML, never the token table's null slot, so the
    /// one place a Ruby-held node becomes a token cannot mint a null kind.
    ///
    /// Safe: both resolvers read the word through a checked lookup
    /// (`Token::arena`, `Token::xml`), so a stale or foreign word is refused,
    /// never undefined behaviour.
    #[inline]
    pub fn token(self, kind: DocKind) -> Token {
        match kind {
            DocKind::Html => Token::arena(self.0),
            DocKind::Xml => Token::xml(self.0),
        }
    }

    /// Node identity as an integer: for `#==`/`#hash` and the wrapper cache's
    /// key. Never read back as a node.
    #[inline]
    pub fn identity(self) -> usize {
        self.0
    }
}

/// A node as a long-lived Ruby wrapper stores it: the opaque [`HtmlNodeKey`]
/// for HTML, the arena [`NodeId`] for XML.
///
/// The HTML variant is what makes the stored handle opaque: it is a private
/// mint (`html::dom`'s), so a wrapper's node can only have come from
/// the document the wrapper marks. The XML variant carries its document stamp
/// for the same reason. Which variant a wrapper holds is decided by its
/// TypedData type, as before.
#[derive(Clone, Copy)]
pub enum NodeHandle {
    Html(HtmlNodeKey),
    Xml(NodeId),
}

/// A live representation-specific node that can become a long-lived wrapper
/// handle. Its cache identity and stored handle are derived from the same
/// value, so callers cannot accidentally pair an unrelated token and handle.
pub(in crate::bridge) trait NodeHandleSource {
    /// The node's index in its document - its key in the document's node
    /// table, a Fixnum on every target - and the document's stamp.
    fn index_and_stamp(&self) -> (u32, u32);
    fn into_handle(self, document: Value) -> NodeHandle;
}

impl NodeHandle {
    /// The HTML key, when this is an HTML node.
    #[inline]
    pub fn html(self) -> Option<HtmlNodeKey> {
        match self {
            NodeHandle::Html(key) => Some(key),
            NodeHandle::Xml(_) => None,
        }
    }

    /// The XML id, when this is an XML node.
    #[inline]
    pub fn xml(self) -> Option<NodeId> {
        match self {
            NodeHandle::Xml(id) => Some(id),
            NodeHandle::Html(_) => None,
        }
    }
}

/// A node wrapper's data: the node. The keepalive Document is the object's
/// one slot (slot 0 of [`NodeType`]).
///
/// The node is owned by the document's arena (HTML or XML), so the wrapper
/// never frees it; the Document in the slot is what keeps it alive, and the
/// slot's mark is the wrapper's whole GC job, which `nupur` does.
#[derive(Clone, Copy)]
pub struct NodeData {
    /// Representation-opaque; read it only through a kind-checked accessor.
    pub node: NodeHandle,
}

impl crate::bridge::typed::Payload for NodeData {}

/// What a node wrapper holds, copied out: the only way it is read
/// ([`read_node`]), since the data is embedded and moves with its object.
#[derive(Clone, Copy)]
pub struct NodeRef {
    pub node: NodeHandle,
    /// The Document the node keeps alive.
    pub document: Value,
}

/// A node wrapper's type: [`NodeData`], and the Document in slot 0.
pub type NodeWrapperType = EmbeddedType<NodeData, 1>;

/// `v`'s node and Document, or Ruby's own `TypeError` when `v` is not of
/// `ty` (or a type deriving from it).
#[inline]
pub fn read_node(ty: &'static NodeWrapperType, v: Value) -> Result<NodeRef, Error> {
    let (data, [document]) = ty.read(v)?;
    Ok(NodeRef {
        node: data.node,
        document,
    })
}

/* Measured: `NodeHandle` fits two words (the `NonNull`/`NonZero` niches carry
 * the discriminant), so a node wrapper's data - the handle and its one slot -
 * is three words, one more than the two-word `NodeWord` + `VALUE` it
 * replaced. Pinned as a growth ratchet: a field that pushes wrapper data past
 * three words fails the build. */
const _: () = assert!(
    crate::bridge::typed::envelope_size::<NodeData, 1>() <= 24,
    "a node wrapper's data grew past three words; re-measure the wrapper cost before raising this"
);

pub static NODE_DATA_TYPE: NodeWrapperType = EmbeddedType::base(c"Makiri::Node");

pub static HTML_NODE_TYPE: NodeWrapperType =
    EmbeddedType::derived(c"Makiri::HTML::Node", &NODE_DATA_TYPE);

pub static XML_NODE_TYPE: NodeWrapperType =
    EmbeddedType::derived(c"Makiri::XML::Node", &NODE_DATA_TYPE);

/// Which representation a wrapped Ruby node is, decided by its TypedData type
/// rather than its Ruby class. A Document, a NodeSet or any non-node is
/// `Other`.
///
/// An enum, not the C's `c_int` codes: a transcribed code (`NODE_KIND_XML = 1`
/// where it was 2) once made `Document#import_node` read every HTML node as an
/// XML one, and copies of those numbers had spread to three files. A `match`
/// over this is checked for exhaustiveness and cannot be off by one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeRepr {
    Html,
    Xml,
    Other,
}

/// Why [`node_token_in`] refused a node.
#[allow(
    clippy::large_enum_variant,
    reason = "an Error holds its message inline (`crate::msg::Msg`), so it is never boxed"
)]
pub enum NotInDocument {
    /// Not a usable Makiri node - the `TypeError` or `Makiri::Error` its
    /// wrapper raised.
    Unusable(Error),
    /// A node of another document.
    Foreign,
}

/// The engine token of `rb_node`, which must be a node of `document` (the
/// document node included).
///
/// The check behind every token minted for one document's engine - an
/// `XPathContext`'s context node - made here once, so a token
/// never names a node of another document. The token's kind is `document`'s
/// own, not one a caller passes alongside.
pub fn node_token_in(ruby: &Ruby, rb_node: Value, document: Value) -> Result<Token, NotInDocument> {
    let node_document = keepalive_document(ruby, rb_node).map_err(NotInDocument::Unusable)?;
    if !crate::bridge::ruby::same_value(node_document, document) {
        return Err(NotInDocument::Foreign);
    }
    let raw = node_raw(ruby, rb_node).map_err(NotInDocument::Unusable)?;
    Ok(raw.token(DocKind::of(document)))
}

/// The kind-AGNOSTIC node word (the base type, so HTML or XML). Only for the
/// few sites where the representation is irrelevant (identity comparison) or
/// already guaranteed by an external same-document check (the XPath context
/// node, a NodeSet of the node's own document).
///
/// The Document branch is kind-aware: an XML Document resolves to its arena's
/// document node, an HTML one to the HTML arena's.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(raw_node_crossing, reason = "a node's word, for identity alone")
)]
pub fn node_raw(ruby: &Ruby, rb_node: Value) -> Result<NodeWord, Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_node, &CLASS_DOCUMENT) {
        DOC_TYPE.get(&rb_node)?; /* TypeError for a non-Document */
        return Ok(match DocKind::of(rb_node) {
            DocKind::Xml => xml_read(&rb_node)?.doc_node().into(),
            DocKind::Html => RawNode::from(html_read(&rb_node)?.doc().as_node()).into(),
        });
    }
    /* TypeError for a non-node, as TypedData_Get_Struct raised. */
    Ok(match read_node(&NODE_DATA_TYPE, rb_node)?.node {
        NodeHandle::Html(key) => NodeWord::from(key.raw_node()),
        NodeHandle::Xml(id) => NodeWord::from(id),
    })
}

/// Which representation `v` wraps ([`NodeRepr`]).
pub fn node_repr(v: Value) -> NodeRepr {
    if HTML_NODE_TYPE.is(v) {
        NodeRepr::Html
    } else if XML_NODE_TYPE.is(v) {
        NodeRepr::Xml
    } else {
        NodeRepr::Other
    }
}

/// Node identity as an integer, for `#==`/`#eql?`/`#hash`/`#pointer_id` -
/// kind-agnostic, and never dereferenced.
pub fn node_identity(ruby: &Ruby, rb_node: Value) -> Result<usize, Error> {
    Ok(node_raw(ruby, rb_node)?.identity())
}

/// Node identity for `==`/`eql?`: the Document AND the word. The word alone
/// does not tell two documents apart - it is a node id packed with its
/// document's 32-bit stamp, and the stamps wrap (every parse, fragment parses
/// included, takes one), so a long-lived document and one made 2^32 documents
/// later can share a stamp; and an XML word can equal an HTML one. The
/// Document is the same object for every node of one document, so comparing
/// it settles both.
pub fn node_key(ruby: &Ruby, rb_node: Value) -> Result<(usize, usize), Error> {
    Ok((
        crate::bridge::ruby::identity_word(keepalive_document(ruby, rb_node)?),
        node_identity(ruby, rb_node)?,
    ))
}

/// The keepalive Document of any node, or the Document itself.
/// `Err(TypeError)` for a non-node.
pub fn keepalive_document(ruby: &Ruby, rb_node: Value) -> Result<Value, Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_node, &CLASS_DOCUMENT) {
        return Ok(rb_node);
    }
    Ok(read_node(&NODE_DATA_TYPE, rb_node)?.document)
}
