//! The HTML node front door: an arena node wrapped for Ruby (its `Makiri::HTML::*`
//! leaf, the one wrapper the node table keeps), a wrapper's key read back, and a
//! key resolved against its Document - the checked way back in.

#![forbid(unsafe_code)]

use super::*;

/* ------------------------------------------------------------------ *
 * the HTML node front door                                           *
 * ------------------------------------------------------------------ */

/// The `Makiri::HTML::*` leaves, by node type.
static HTML_NODE_CLASSES: NodeClasses = NodeClasses {
    node: &CLASS_HTML_NODE,
    element: &CLASS_HTML_ELEMENT,
    attr: &CLASS_HTML_ATTR,
    text: &CLASS_HTML_TEXT,
    comment: &CLASS_HTML_COMMENT,
    cdata: &CLASS_HTML_CDATA_SECTION,
    pi: &CLASS_HTML_PROCESSING_INSTRUCTION,
    doctype: &CLASS_HTML_DOCUMENT_TYPE,
    fragment: &CLASS_HTML_DOCUMENT_FRAGMENT,
};

/// Wrap a live HTML node handle into its `Makiri::HTML::*` leaf.
///
/// A DOCUMENT node maps back onto the Ruby Document rather than getting a
/// second wrapper; a node type with no specific leaf (entity/notation, which
/// the HTML parser does not produce) falls back to `Makiri::HTML::Node`.
pub fn wrap_html_node(ruby: &Ruby, node: RawNode, document: Value) -> Result<Value, Error> {
    // The caller hands `node` with the Document it belongs to.
    let node_type = html_read(&doc_known(document))?.node(node).node_type();
    wrap_html_node_of_type(ruby, node, node_type, document)
}

/// [`wrap_html_node`] for a node that must be one of `document`'s: an
/// `InternalError` when it is not (another document's, or a stale word).
pub(in crate::bridge) fn wrap_html_node_checked(
    ruby: &Ruby,
    node: RawNode,
    document: Value,
) -> Result<Value, Error> {
    /* A wrapper in the table was made for this word, checked then - and a
     * word names one node of one document for the document's life - so a
     * hit needs no check, no type and no class. */
    if let Some(cached) = crate::bridge::wrapper::cached_node(document, &node)? {
        return Ok(cached);
    }
    let node_type = html_read(&doc_known(document))?
        .doc()
        .checked(node)
        .map(|n| n.node_type())
        .ok_or_else(|| crate::bridge::ruby::internal_error("a stored node of another document"))?;
    wrap_html_node_of_type(ruby, node, node_type, document)
}

/// [`wrap_html_node`] for a caller that read the node's type already, in the
/// read it found the node in - one read the fewer.
pub(super) fn wrap_html_node_of_type(
    ruby: &Ruby,
    node: RawNode,
    node_type: NodeType,
    document: Value,
) -> Result<Value, Error> {
    if node_type == NodeType::Document {
        return Ok(document);
    }
    let klass = HTML_NODE_CLASSES.class_for(ruby, node_type)?;
    crate::bridge::wrapper::wrap_cached(ruby, &HTML_NODE_TYPE, klass, node, document)
}

impl crate::bridge::wrapper::NodeHandleSource for RawNode {
    fn index_and_stamp(&self) -> (u32, u32) {
        (self.index(), self.stamp())
    }

    fn into_handle(self, document: Value) -> NodeHandle {
        NodeHandle::Html(mint_html_key(self, document))
    }
}

/// The key for `node` under `document`, minted only when a wrapper is actually
/// built. Its caller vouches `node` is a node of `document`, so a refusal is a
/// broken invariant, not a bad input: a panic, which the trampoline raises as
/// `InternalError` (or, were an exit pending, resumes that first).
#[allow(
    clippy::expect_used,
    reason = "wrap_html_node's callers vouch the node belongs to the document"
)]
fn mint_html_key(node: RawNode, document: Value) -> HtmlNodeKey {
    /* `wrap_html_node` read the document a moment ago, with nothing run
     * between, so it can be read again. */
    with_html_parsed_known(document, |parsed| parsed.mint_key(node))
        .expect("wrap_html_node: the document must be readable again")
        .expect("wrap_html_node: the node must belong to the document it is wrapped under")
}

fn uninitialized() -> Error {
    crate::bridge::ruby::type_error("uninitialized HTML node")
}

/// The key and keepalive Document of an HTML node or HTML Document.
///
/// `Err(TypeError)` for an XML node or Document: the typed-data check is
/// against [`HTML_NODE_TYPE`], which an XML node (wrapped under
/// `XML_NODE_TYPE`) does not satisfy. The Document node is keyed too: it is its own `owner_document`, so
/// `mint_key` accepts it.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(raw_node_crossing, reason = "a Document's own node, minted as its key")
)]
pub(in crate::bridge) fn html_node_key(
    ruby: &Ruby,
    rb_node: Value,
) -> Result<(HtmlNodeKey, Value), Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_node, &CLASS_DOCUMENT) {
        if crate::bridge::ruby::is_kind_of(ruby, rb_node, &CLASS_XML_DOCUMENT) {
            return Err(crate::bridge::ruby::type_error(
                "expected an HTML node, got a Makiri::XML::Document",
            ));
        }
        html_doc_check(rb_node)?;
        let key = with_html_parsed_known(rb_node, |parsed| {
            parsed.mint_key(RawNode::from(parsed.doc().as_node()))
        })?
        .map_err(|ForeignNode| foreign_node_error())?;
        return Ok((key, rb_node));
    }
    let nd = crate::bridge::wrapper::read_node(&HTML_NODE_TYPE, rb_node)?;
    let key = nd.node.html().ok_or_else(uninitialized)?;
    Ok((key, nd.document))
}

/// [`wrap_html_node`] for an optional handle.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "a node wrapped: the wrapper keeps its key, and its document alive"
    )
)]
pub fn wrap_node(
    ruby: &Ruby,
    node: Option<HtmlNode<'_>>,
    document: Value,
) -> Result<Option<Value>, Error> {
    node.map(|n| wrap_html_node(ruby, RawNode::from(n), document))
        .transpose()
}

/* ------------------------------------------------------------------ *
 * resolving a long-lived key                                          *
 * ------------------------------------------------------------------ */

/// A key paired with the wrong Document: an internal invariant broken, not a
/// bad argument, so `InternalError` rather than `Makiri::Error`.
pub(super) fn foreign_node_error() -> Error {
    Error::new(
        &crate::init::EXC_INTERNAL_ERROR,
        "an HTML node key belongs to a different document",
    )
}

/// Run `f` over the node `key` names, after checking that node belongs to
/// `rb_doc`'s document.
///
/// `rb_doc` is the node's keepalive Document, and `key` was minted from a node
/// of that document, so the check fails only if a caller paired a key with the
/// wrong Document - hence `InternalError`. `f` runs inside the document borrow,
/// so it must not run Ruby that could re-enter this document, and must not
/// return a borrowed handle or slice.
pub(in crate::bridge) fn with_html_node<R>(
    rb_doc: Value,
    key: HtmlNodeKey,
    f: impl FnOnce(HtmlNode<'_>) -> R,
) -> Result<R, Error> {
    with_html_parsed_known(rb_doc, |parsed| {
        parsed
            .resolve(key)
            .map(f)
            .map_err(|ForeignNode| foreign_node_error())
    })?
}

/// The node `key` names, as a reference into `rb_doc`'s document, after the
/// same check [`with_html_node`] makes.
///
/// What an edit takes for a node of the document it is changing: the edit
/// holds that document by `&mut` (`HtmlDocMut`), so the node is read through
/// that borrow - never through a second handle - and a reference is all that
/// crosses. The one place a checked key leaves the bridge's handles as a
/// reference.
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        raw_node_crossing,
        reason = "a checked key as a reference into its document, for an edit that holds it"
    )
)]
pub(in crate::bridge) fn node_ref(rb_doc: Value, key: HtmlNodeKey) -> Result<RawNode, Error> {
    with_html_node(rb_doc, key, |n| RawNode::from(n))
}
