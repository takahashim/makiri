//! A method's XML receiver (`XmlSelf`) and its arena read (`XmlView`): what the
//! readers read through, and the receiver's child lists.

#![forbid(unsafe_code)]

use super::*;

impl ChildListHost for XmlSelf {
    type Tree<'a> = XmlDoc;
    fn document(&self) -> Value {
        self.document()
    }
    fn with_children<R>(
        &self,
        list: ChildList,
        f: impl for<'a> FnOnce(Children<'a, XmlDoc>) -> R,
    ) -> Result<R, Error> {
        let view = self.read()?;
        Ok(f(Children {
            tree: &view,
            parent: self.id,
            list,
        }))
    }
    fn wrap_child(&self, ruby: &Ruby, token: Option<usize>) -> Result<Option<Value>, Error> {
        token
            .and_then(NodeId::from_token)
            .map(|id| wrap_xml_node(ruby, id, self.document()))
            .transpose()
    }
}

/* ------------------------------------------------------------------ *
 * the XML receiver and the node front door                           *
 * ------------------------------------------------------------------ */

/// A method receiver already checked to be an XML node or XML Document; see the
/// HTML twin, `HtmlSelf`.
pub struct XmlSelf {
    pub value: Value,
    pub id: NodeId,
    /// The keepalive Document (the receiver itself for a Document), as the
    /// handle its content is read through - and written, by `Editing`.
    pub(super) document: DocHandle,
}

impl crate::bridge::rb::TryConvert for XmlSelf {
    fn try_convert(ruby: &Ruby, value: Value) -> Result<Self, Error> {
        /* One class test and one read of the wrapper: the node and its
         * Document come from the same place ([`xml_node_unwrap`] and
         * [`xml_node_document`] each ask, for the callers that need one). */
        if crate::bridge::ruby::is_kind_of(ruby, value, &CLASS_XML_DOCUMENT) {
            /* TypeError for any other Document */
            let document = XML_DOC_TYPE.check(value)?;
            let id = xml_read(&document)?.doc_node();
            return Ok(XmlSelf {
                value,
                id,
                document,
            });
        }
        let nd = crate::bridge::wrapper::read_node(&XML_NODE_TYPE, value)?;
        Ok(XmlSelf {
            value,
            id: nd.node.xml().ok_or_else(wrapper_without_node)?,
            document: doc_known(nd.document),
        })
    }
}

impl XmlSelf {
    /// The keepalive Document (the receiver itself for a Document).
    #[inline]
    pub fn document(&self) -> Value {
        self.document.value()
    }

    /// The arena behind the receiver's Document, borrowed for reading: a
    /// counted borrow, which an edit of the same document refuses while it
    /// is held. A Ruby result may be built with it held: every allocation
    /// that can raise returns `Err`, so the guard drops on the way out
    /// ([`XmlView::set`]). The receiver is a method argument, which keeps
    /// its Document alive; the view borrows `self`, so it ends with the call.
    pub fn read(&self) -> Result<XmlView<'_>, Error> {
        Ok(XmlView {
            read: xml_read(&self.document)?,
            id: self.id,
            document: self.document(),
        })
    }
}

/// A receiver's arena borrowed for reading, with the receiver's node:
/// [`XmlSelf::read`]. Reads as the arena.
pub struct XmlView<'a> {
    read: XmlRead<'a>,
    /// The receiver's node.
    pub id: NodeId,
    document: Value,
}

impl core::ops::Deref for XmlView<'_> {
    type Target = XmlDoc;
    #[inline]
    fn deref(&self) -> &XmlDoc {
        &self.read
    }
}

impl XmlView<'_> {
    /// The node `f` finds from the receiver's, wrapped. The read ends before
    /// the wrap, which needs nothing of it but the node's id and type.
    pub fn wrap(
        self,
        ruby: &Ruby,
        f: impl FnOnce(&XmlDoc, NodeId) -> Option<NodeId>,
    ) -> Result<Option<Value>, Error> {
        let found = f(&self, self.id).map(|n| (n, self.type_(n)));
        let document = self.document;
        drop(self);
        found
            .map(|(n, ty)| wrap_xml_node_of_type(ruby, n, ty, document))
            .transpose()
    }

    /// The nodes `f` lists from the receiver's, as a NodeSet built with the
    /// read held ([`node_set_from_read`](crate::bridge::node_set::node_set_from_read)).
    /// `f` hands each node to the sink it is given.
    pub fn set(
        self,
        ruby: &Ruby,
        f: impl Fn(&XmlDoc, NodeId, &mut dyn FnMut(NodeId)),
    ) -> Result<Value, Error> {
        let document = self.document;
        crate::bridge::node_set::node_set_from_read(ruby, self, document, |view, out| {
            f(view, view.id, &mut |n| out(n.into()));
        })
    }
}
