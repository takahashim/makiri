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

/// Wrap an arena node into its `Makiri::XML::*` leaf.
///
/// The DOCUMENT node maps back onto the Ruby Document rather than getting a
/// second wrapper, so the arena has exactly one owner. The id resolves through
/// `document`'s arena; a caller with no node holds an `Option` and maps `None`
/// to nil itself.
pub fn wrap_xml_node(ruby: &Ruby, id: NodeId, document: Value) -> Result<Value, Error> {
    let ty = xml_read(&doc_known(document))?.type_(id);
    wrap_xml_node_of_type(ruby, id, ty, document)
}

/// [`wrap_xml_node`] for a node that must be one of `document`'s: an
/// `InternalError` when the arena has no node by that id.
pub(in crate::bridge) fn wrap_xml_node_checked(
    ruby: &Ruby,
    id: NodeId,
    document: Value,
) -> Result<Value, Error> {
    /* A hit was checked when it was made, as `wrap_html_node_checked`'s. */
    if let Some(cached) = crate::bridge::wrapper::cached_node(document, &id)? {
        return Ok(cached);
    }
    let ty = xml_read(&doc_known(document))?.type_(id);
    if ty.is_none() {
        return Err(crate::bridge::ruby::internal_error(
            "a stored node of another document",
        ));
    }
    wrap_xml_node_of_type(ruby, id, ty, document)
}

/// [`wrap_xml_node`] for a caller that read the node's type already, in the
/// read it found the node in - one read the fewer.
pub(super) fn wrap_xml_node_of_type(
    ruby: &Ruby,
    id: NodeId,
    ty: Option<ArenaKind>,
    document: Value,
) -> Result<Value, Error> {
    if ty == Some(ArenaKind::Document) {
        return Ok(document);
    }
    let klass = XML_NODE_CLASSES.class_for(ruby, ty.map_or(CrateKind::Other, Into::into))?;

    crate::bridge::wrapper::wrap_cached(ruby, &XML_NODE_TYPE, klass, id, document)
}

impl crate::bridge::wrapper::NodeHandleSource for NodeId {
    fn index_and_stamp(&self) -> (u32, u32) {
        (self.index(), self.stamp())
    }

    fn into_handle(self, _document: Value) -> NodeHandle {
        NodeHandle::Xml(self)
    }
}

/// The arena node behind a wrapper.
///
/// An XML Document resolves to its arena's DOCUMENT node. Anything else goes
/// through the XML TypedData type, which fails with TypeError for an HTML node.
pub fn xml_node_unwrap(ruby: &Ruby, rb_self: Value) -> Result<NodeId, Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_self, &CLASS_XML_DOCUMENT) {
        let document = XML_DOC_TYPE.check(rb_self)?; /* TypeError for any other Document */
        return Ok(xml_read(&document)?.doc_node());
    }
    let nd = crate::bridge::wrapper::read_node(&XML_NODE_TYPE, rb_self)?;
    /* A wrapper's word is a real node; a null one is a broken invariant,
     * answered as `InternalError` directly rather than by a panic (which the
     * trampoline would also make one) - `XmlSelf::try_convert` reads it. */
    nd.node.xml().ok_or_else(wrapper_without_node)
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

/// The keepalive Document of an XML node. XML-strict: it rejects an HTML node
/// at the type boundary, like [`xml_node_unwrap`].
pub fn xml_node_document(ruby: &Ruby, rb_self: Value) -> Result<Value, Error> {
    if crate::bridge::ruby::is_kind_of(ruby, rb_self, &CLASS_XML_DOCUMENT) {
        return Ok(rb_self);
    }
    Ok(crate::bridge::wrapper::read_node(&XML_NODE_TYPE, rb_self)?.document)
}
