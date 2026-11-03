//! The XML node front door: an arena node wrapped for Ruby (its `Makiri::XML::*`
//! leaf, the one wrapper the node table keeps), and a wrapper read back as its
//! node and Document.

#![forbid(unsafe_code)]

use super::*;

/* ------------------------------------------------------------------ *
 * the XML node front door                                            *
 * ------------------------------------------------------------------ */

/// The `Makiri::XML::*` leaves, by node type.
static XML_NODE_CLASSES: NodeClasses = NodeClasses {
    node: &CLASS_XML_NODE,
    element: &CLASS_XML_ELEMENT,
    attr: &CLASS_XML_ATTR,
    text: &CLASS_XML_TEXT,
    comment: &CLASS_XML_COMMENT,
    cdata: &CLASS_XML_CDATA_SECTION,
    pi: &CLASS_XML_PROCESSING_INSTRUCTION,
    doctype: &CLASS_XML_DOCUMENT_TYPE,
    fragment: &CLASS_XML_DOCUMENT_FRAGMENT,
};

/// Wrap an XML node into its `Makiri::XML::*` leaf
/// ([`wrap_dom_node`](crate::bridge::wrapper::wrap_dom_node)): the Document node maps
/// back onto the Ruby Document, so the arena has exactly one owner, and an id
/// `document`'s arena does not have is `InternalError`. A caller with no node
/// holds an `Option` and maps `None` to nil itself.
pub fn wrap_xml_node(ruby: &Ruby, id: NodeId, document: Value) -> Result<Value, Error> {
    crate::bridge::wrapper::wrap_dom_node(ruby, id, document)
}

/// [`wrap_xml_node`] for a caller that read the node's type already, in the
/// read it found the node in - one read the fewer.
pub(super) fn wrap_xml_node_of_type(
    ruby: &Ruby,
    id: NodeId,
    ty: Option<ArenaKind>,
    document: Value,
) -> Result<Value, Error> {
    /* A node the arena does not have is a broken invariant, refused rather
     * than wrapped as a plain node. */
    let ty =
        ty.ok_or_else(|| crate::bridge::ruby::internal_error("a node its document does not have"))?;
    crate::bridge::wrapper::wrap_dom_node_of_type(ruby, id, ty.into(), document)
}

impl crate::bridge::wrapper::NodeHandleSource for NodeId {
    fn index_and_stamp(&self) -> (u32, u32) {
        (self.index(), self.stamp())
    }

    fn into_handle(self, _document: Value) -> NodeHandle {
        NodeHandle::Xml(self)
    }

    fn wrapper_type() -> &'static crate::bridge::wrapper::NodeWrapperType {
        &XML_NODE_TYPE
    }

    fn classes() -> &'static crate::bridge::wrapper::NodeClasses {
        &XML_NODE_CLASSES
    }

    fn node_type(self, document: Value) -> Result<Option<crate::node_type::NodeType>, Error> {
        Ok(xml_read(&doc_known(document))?.type_(self).map(Into::into))
    }
}

/// The arena node behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node.
pub fn xml_node_unwrap(ruby: &Ruby, rb_self: Value) -> Result<NodeId, Error> {
    xml_node_ref(ruby, rb_self).map(|(id, _)| id)
}

/// [`xml_node_unwrap`] and the node's keepalive Document, in one read of the
/// wrapper - the XML twin of `html_node_key`.
pub fn xml_node_ref(ruby: &Ruby, rb_self: Value) -> Result<(NodeId, Value), Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_self, &CLASS_XML_DOCUMENT) {
        let document = XML_DOC_TYPE.check(rb_self)?; /* TypeError for any other Document */
        return Ok((xml_read(&document)?.doc_node(), rb_self));
    }
    let nd = crate::bridge::wrapper::read_node(&XML_NODE_TYPE, rb_self)?;
    /* A wrapper's word is a real node; a null one is a broken invariant,
     * answered as `InternalError` directly rather than by a panic (which the
     * trampoline would also make one) - `XmlSelf::try_convert` reads it. */
    let id = nd.node.xml().ok_or_else(wrapper_without_node)?;
    Ok((id, nd.document))
}

pub(super) fn wrapper_without_node() -> Error {
    Error::new(
        &crate::init::EXC_INTERNAL_ERROR,
        "an XML node wrapper without its node",
    )
}

/// `Err(TypeError)` unless `rb_doc` is an XML Document.
pub fn xml_doc_check(rb_doc: Value) -> Result<(), Error> {
    XML_DOC_TYPE.ensure(rb_doc)
}
