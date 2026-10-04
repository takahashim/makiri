//! Copying a node or a subtree, within one document or between two.
//!
//! A copy READS one arena and WRITES another - or, for `clone_node`, the same
//! one, which Rust cannot express as `(&mut Document, &Document)`. Rather than
//! keep two copies of every routine (one per source), a copy lifts the node's
//! fields OUT of the source first ([`CopiedNode::read`]) and writes them back
//! ([`CopiedNode::write`]). The fields had to be owned anyway - a `&mut Document`
//! cannot be held across a read of its own byte store - so the split costs
//! nothing and leaves one body per operation.

#![forbid(unsafe_code)]

use super::copy_span;
use crate::falloc::{OomResult, VecPush};
use crate::xml::qname::Split;
use crate::xml::{ArenaKind, AttrNs, Document, MutError, NodeFlags, NodeId, Span};

/// A copied `value` span. XML distinguishes "never set" from "set to empty",
/// so a copy has to carry the difference.
enum CopiedValue {
    Absent,
    Empty,
    Bytes(Vec<u8>),
}

impl CopiedValue {
    fn read(doc: &Document, span: Span) -> Result<CopiedValue, MutError> {
        Ok(if span.len > 0 {
            CopiedValue::Bytes(copy_span(doc.span(span))?)
        } else if span.is_absent() {
            CopiedValue::Absent
        } else {
            CopiedValue::Empty
        })
    }

    /// The span to store: ABSENT stays absent, the rest land in `dst`'s bytes.
    fn write(&self, dst: &mut Document) -> Result<Span, MutError> {
        match self {
            CopiedValue::Absent => Ok(Span::ABSENT),
            CopiedValue::Empty => Ok(Span::EMPTY),
            CopiedValue::Bytes(v) => dst.store(v).map_err(MutError::from),
        }
    }
}

/// A DOCTYPE's name and ids, owned - read by `Document::doctype_ids` and
/// written by `Document::new_doctype`, the arena's one reader and writer of the
/// fields a DOCTYPE repurposes. An id is `None` when omitted and `Some(empty)`
/// for `PUBLIC ""`, which `new_doctype` stores as present.
struct CopiedDoctype {
    name: Vec<u8>,
    public: Option<Vec<u8>>,
    system: Option<Vec<u8>>,
}

/// One node's own fields, owned, out of any arena. Attributes come with it,
/// since they are part of the node's identity rather than its children.
struct CopiedNode {
    type_: ArenaKind,
    /// The qualified name and its split, for a node that has one.
    qname: Option<(Vec<u8>, Split)>,
    /// A bare local name (a PI target, a doctype name) on a node with no qname.
    local: Option<Vec<u8>>,
    value: CopiedValue,
    /// A DOCTYPE's own fields, which it keeps in the name fields: copied
    /// through the arena's DOCTYPE accessors rather than as names. `None`
    /// for every other kind.
    doctype: Option<CopiedDoctype>,
    ns_uri: Option<Vec<u8>>,
    flags: NodeFlags,
    /// An attribute's namespace state; `Derived` for anything else. Copied
    /// with `flags` - it lived in them until it became a field of its own.
    attr_ns: AttrNs,
    attrs: Vec<CopiedNode>,
}

impl CopiedNode {
    /// Lift `src`'s own fields and attributes out of `doc`.
    fn read(doc: &Document, src: NodeId) -> Result<CopiedNode, MutError> {
        let Some(node) = doc.try_node(src) else {
            return Err(MutError::Type);
        };
        let (type_, flags, attr_ns) = (node.type_, node.flags, node.attr_ns);
        let (qname_span, local_span, value_span, ns_span) =
            (node.qname, node.local, node.value, node.ns_uri);

        /* A DOCTYPE repurposes the name fields for its ids, so it is read by
         * kind (`doctype_ids`) and never as a name: splitting its "qname" once
         * read the PUBLIC id's length as a prefix length and copied the name's
         * bytes, plus whatever followed them in the store, as the id. */
        if type_ == ArenaKind::DocumentType {
            let ids = doc.doctype_ids(src).ok_or(MutError::Type)?;
            let owned = |v: Option<&[u8]>| v.map(copy_span).transpose();
            return Ok(CopiedNode {
                type_,
                qname: None,
                local: None,
                value: CopiedValue::Absent,
                doctype: Some(CopiedDoctype {
                    name: copy_span(doc.local(src))?,
                    public: owned(ids.public)?,
                    system: owned(ids.system)?,
                }),
                ns_uri: None,
                flags,
                attr_ns,
                attrs: Vec::new(),
            });
        }

        let qname = if qname_span.len > 0 {
            Some((copy_span(doc.qname(src))?, doc.split_of(src)))
        } else {
            None
        };
        let local = if qname_span.len == 0 && local_span.len > 0 {
            Some(copy_span(doc.local(src))?)
        } else {
            None
        };
        let value = CopiedValue::read(doc, value_span)?;
        let ns_uri = if ns_span.len > 0 {
            Some(copy_span(doc.ns(src))?)
        } else {
            None
        };

        let mut attrs: Vec<CopiedNode> = Vec::new();
        for attr in doc.attributes(src) {
            attrs
                .falloc_push(CopiedNode::read(doc, attr)?)
                .or_oom::<MutError>()?;
        }

        Ok(CopiedNode {
            type_,
            qname,
            local,
            value,
            doctype: None,
            ns_uri,
            flags,
            attr_ns,
            attrs,
        })
    }

    /// A fresh node of any kind but DOCTYPE, its name and value written.
    fn write_fields(&self, dst: &mut Document) -> Result<NodeId, MutError> {
        let n = dst.new_node(self.type_)?;
        if let Some((name, sp)) = &self.qname {
            dst.assign_qname(n, name, sp.prefix_len, sp.local_off, sp.local_len)?;
        } else if let Some(local) = &self.local {
            let span = dst.store(local)?;
            dst.node_mut(n).local = span;
        }
        let value = self.value.write(dst)?;
        dst.node_mut(n).value = value;
        Ok(n)
    }

    /// Write these fields as a fresh, detached node in `dst`.
    fn write(&self, dst: &mut Document) -> Result<NodeId, MutError> {
        let n = match &self.doctype {
            Some(dt) => dst.new_doctype(&dt.name, dt.public.as_deref(), dt.system.as_deref())?,
            None => self.write_fields(dst)?,
        };
        let node = dst.node_mut(n);
        node.flags = self.flags;
        node.attr_ns = self.attr_ns;
        if let Some(uri) = &self.ns_uri {
            let span = dst.store(uri)?;
            dst.node_mut(n).ns_uri = span;
        }
        /* attributes, in order */
        let mut tail: Option<NodeId> = None;
        for attr in &self.attrs {
            let ca = attr.write(dst)?;
            dst.link_attr(n, tail, ca);
            tail = Some(ca);
        }
        Ok(n)
    }
}

/// The document a copy reads. `None` means the destination itself, which is
/// what a same-document [`clone_node`] needs: the caller re-borrows its
/// `&mut Document` as shared for each read, and this is the one line that says
/// so instead of a second copy of every routine.
type ReadFrom<'a> = Option<&'a Document>;

#[inline]
fn source<'a>(dst: &'a Document, from: ReadFrom<'a>) -> &'a Document {
    from.unwrap_or(dst)
}

/// One arena copy of `src` - own fields and attributes, NOT children.
fn copy_one(dst: &mut Document, from: ReadFrom<'_>, src: NodeId) -> Result<NodeId, MutError> {
    let copied = CopiedNode::read(source(dst, from), src)?;
    copied.write(dst)
}

/// Deep copy of `src`'s subtree (iterative; no recursion, so a deep tree cannot
/// exhaust the stack).
fn deep_copy(dst: &mut Document, from: ReadFrom<'_>, src: NodeId) -> Result<NodeId, MutError> {
    let root = copy_one(dst, from, src)?;
    let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
    stack.falloc_push((src, root)).or_oom::<MutError>()?;
    while let Some((s, d)) = stack.pop() {
        let mut sc = source(dst, from).first_child(s);
        while let Some(child) = sc {
            let dc = copy_one(dst, from, child)?;
            dst.append_child(d, dc);
            if source(dst, from).first_child(child).is_some() {
                stack.falloc_push((child, dc)).or_oom::<MutError>()?;
            }
            sc = source(dst, from).next(child);
        }
    }
    Ok(root)
}

/// The cross-document entries take two distinct documents; a same-document copy
/// is [`clone_node`], which reads and writes one arena.
#[inline]
fn debug_assert_distinct(dst: &Document, src_doc: &Document) {
    debug_assert!(
        !core::ptr::eq(dst as *const Document, src_doc as *const Document),
        "same-document import must use clone_node, not the cross-document copy"
    );
}

/// Cross-document deep import (`importNode`).
pub fn import_subtree(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
) -> Result<NodeId, MutError> {
    debug_assert_distinct(dst, src_doc);
    deep_copy(dst, Some(src_doc), src)
}

/// Cross-document `copyNode`: shallow or deep, source in `src_doc`.
pub fn copy_node_from(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
    deep: bool,
) -> Result<NodeId, MutError> {
    debug_assert_distinct(dst, src_doc);
    if deep {
        deep_copy(dst, Some(src_doc), src)
    } else {
        copy_one(dst, Some(src_doc), src)
    }
}

/// Same-document `cloneNode`: shallow or deep, reading the arena it writes.
pub fn clone_node(doc: &mut Document, src: NodeId, deep: bool) -> Result<NodeId, MutError> {
    if deep {
        deep_copy(doc, None, src)
    } else {
        copy_one(doc, None, src)
    }
}

/// A copy of the whole of `src` as a new document - `Document#dup` - node for
/// node rather than through its markup: the top-level children in their order
/// (the DOCTYPE, comments and PIs around the root included), every QName,
/// namespace URI and namespace state as `src` holds them, attributes with their
/// provenance, character data as stored.
///
/// What a re-parse of `to_xml` decided instead is stated here:
/// - no namespace is resolved again - every element of a document is decided,
///   and the copy carries the decision (a re-parse gave an attribute set in a
///   namespace with no prefix an invented `ns1:` and a declaration for it);
/// - nothing has to be WRITABLE: data the DOM holds and XML cannot write is
///   copied, where the re-parse failed on it;
/// - the byte and node budgets are `src`'s, not the default - its content
///   fits them - and whether it carried an `encoding` declaration too;
/// - no source position: a copied node was not parsed.
pub fn copy_document(src: &Document) -> Result<Box<Document>, MutError> {
    let mut dst = Document::create(None)?;
    dst.inherit_meta(src);
    let doc_node = dst.doc_node();
    let mut child = src.first_child(src.doc_node());
    while let Some(c) = child {
        let copy = deep_copy(&mut dst, Some(src), c)?;
        dst.append_child(doc_node, copy);
        child = src.next(c);
    }
    dst.sync_doc_meta(doc_node);
    Ok(dst)
}
