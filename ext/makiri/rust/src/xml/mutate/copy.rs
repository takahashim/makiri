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
use crate::falloc::Reserve;
use crate::xml::qname::Split;
use crate::xml::{Document, MutStatus, NodeId, NodeType, Span};

/// A copied `value` span. XML distinguishes "never set" from "set to empty" - a
/// doctype's `PUBLIC ""` is present - so a copy has to carry the difference.
enum CopiedValue {
    Absent,
    Empty,
    Bytes(Vec<u8>),
}

/// One node's own fields, owned, out of any arena. Attributes come with it,
/// since they are part of the node's identity rather than its children.
struct CopiedNode {
    type_: NodeType,
    /// The qualified name and its split, for a node that has one.
    qname: Option<(Vec<u8>, Split)>,
    /// A bare local name (a PI target, a doctype name) on a node with no qname.
    local: Option<Vec<u8>>,
    value: CopiedValue,
    ns_uri: Option<Vec<u8>>,
    flags: u32,
    attrs: Vec<CopiedNode>,
}

impl CopiedNode {
    /// Lift `src`'s own fields and attributes out of `doc`.
    fn read(doc: &Document, src: NodeId) -> Result<CopiedNode, MutStatus> {
        let Some(node) = doc.try_node(src) else {
            return Err(MutStatus::Type);
        };
        let (type_, flags) = (node.type_, node.flags);
        let (qname_span, local_span, value_span, ns_span) =
            (node.qname, node.local, node.value, node.ns_uri);

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
        let value = if value_span.len > 0 {
            CopiedValue::Bytes(copy_span(doc.value(src))?)
        } else if value_span.is_absent() {
            CopiedValue::Absent
        } else {
            CopiedValue::Empty
        };
        let ns_uri = if ns_span.len > 0 {
            Some(copy_span(doc.ns(src))?)
        } else {
            None
        };

        let mut attrs: Vec<CopiedNode> = Vec::new();
        let mut a = doc.attrs(src);
        while let Some(attr) = a {
            attrs.falloc_reserve(1).map_err(|_| MutStatus::Oom)?;
            attrs.push(CopiedNode::read(doc, attr)?);
            a = doc.next(attr);
        }

        Ok(CopiedNode {
            type_,
            qname,
            local,
            value,
            ns_uri,
            flags,
            attrs,
        })
    }

    /// Write these fields as a fresh, detached node in `dst`.
    fn write(&self, dst: &mut Document) -> Result<NodeId, MutStatus> {
        let n = dst.new_node(self.type_).map_err(|_| MutStatus::Oom)?;
        if let Some((name, sp)) = &self.qname {
            if dst
                .assign_qname(n, name, sp.prefix_len, sp.local_off, sp.local_len)
                .is_err()
            {
                return Err(MutStatus::Oom);
            }
        } else if let Some(local) = &self.local {
            let span = dst.store(local).map_err(|_| MutStatus::Oom)?;
            dst.node_mut(n).local = span;
        }
        match &self.value {
            CopiedValue::Absent => {}
            CopiedValue::Empty => dst.node_mut(n).value = Span::EMPTY,
            CopiedValue::Bytes(v) => {
                let span = dst.store(v).map_err(|_| MutStatus::Oom)?;
                dst.node_mut(n).value = span;
            }
        }
        dst.node_mut(n).flags = self.flags;
        if let Some(uri) = &self.ns_uri {
            let span = dst.store(uri).map_err(|_| MutStatus::Oom)?;
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
fn copy_one(dst: &mut Document, from: ReadFrom<'_>, src: NodeId) -> Result<NodeId, MutStatus> {
    let copied = CopiedNode::read(source(dst, from), src)?;
    copied.write(dst)
}

/// Deep copy of `src`'s subtree (iterative; no recursion, so a deep tree cannot
/// exhaust the stack).
fn deep_copy(dst: &mut Document, from: ReadFrom<'_>, src: NodeId) -> Result<NodeId, MutStatus> {
    let root = copy_one(dst, from, src)?;
    let mut stack: Vec<(NodeId, NodeId)> = Vec::new();
    stack.falloc_reserve(1).map_err(|_| MutStatus::Oom)?;
    stack.push((src, root));
    while let Some((s, d)) = stack.pop() {
        let mut sc = source(dst, from).first_child(s);
        while let Some(child) = sc {
            let dc = copy_one(dst, from, child)?;
            dst.append_child(d, dc);
            if source(dst, from).first_child(child).is_some() {
                stack.falloc_reserve(1).map_err(|_| MutStatus::Oom)?;
                stack.push((child, dc));
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
) -> Result<NodeId, MutStatus> {
    debug_assert_distinct(dst, src_doc);
    deep_copy(dst, Some(src_doc), src)
}

/// Cross-document `copyNode`: shallow or deep, source in `src_doc`.
pub fn copy_node_from(
    dst: &mut Document,
    src_doc: &Document,
    src: NodeId,
    deep: bool,
) -> Result<NodeId, MutStatus> {
    debug_assert_distinct(dst, src_doc);
    if deep {
        deep_copy(dst, Some(src_doc), src)
    } else {
        copy_one(dst, Some(src_doc), src)
    }
}

/// Same-document `cloneNode`: shallow or deep, reading the arena it writes.
pub fn clone_node(doc: &mut Document, src: NodeId, deep: bool) -> Result<NodeId, MutStatus> {
    if deep {
        deep_copy(doc, None, src)
    } else {
        copy_one(doc, None, src)
    }
}

/* ---- insertion ---- */
