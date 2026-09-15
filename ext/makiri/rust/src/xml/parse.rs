//! Document creation and XML parsing entry points.

use crate::xml::tree;
use crate::xml::{Document, Limits, NodeId, Status, MAX_BYTES};

pub fn xml_doc_new() -> Result<Box<Document>, Status> {
    Document::create(None, 0)
}

pub fn xml_doc_destroy(doc: Box<Document>) {
    drop(doc);
}

pub fn xml_doc_memsize(doc: &Document) -> usize {
    doc.memsize()
}

pub fn xml_preorder_next(doc: &Document, root: NodeId, cur: NodeId) -> NodeId {
    doc.preorder_next(root, cur).unwrap_or(NodeId::INVALID)
}

pub fn xml_parse(src: &[u8]) -> Result<Box<Document>, Status> {
    xml_parse_ex(src, None)
}

pub fn xml_parse_ex(src: &[u8], limits: Option<&Limits>) -> Result<Box<Document>, Status> {
    let lim = limits.map(|l| l.max_bytes);
    let max = lim.filter(|&n| n != 0).unwrap_or(MAX_BYTES);
    if src.len() > max {
        return Err(Status::Limit);
    }
    tree::parse_ex(src, lim)
}

pub fn xml_parse_fragment(
    doc: &mut Document,
    src: &[u8],
    inherit_doc_ns: bool,
) -> Result<NodeId, Status> {
    if src.len() > doc.max_bytes {
        return Err(Status::Limit);
    }
    tree::parse_fragment(doc, src, inherit_doc_ns)
}
