//! A Lexbor document copied into the parser port's arena
//! (`crate::html::dom`) - stage 2 of `docs/rewrite_parser.ja.md`, where the
//! arena is filled from Lexbor's parse before the tree builder is ported.
//!
//! The copy reads Lexbor only through the adapter's typed handles and writes
//! what they report: kinds, names as written, namespaces and prefixes,
//! attributes in order with the `id`/`class` shortcuts, character data,
//! template contents, source offsets and the quirks mode. Names are interned
//! again in the arena's own tables, so a static id stays the same number and
//! an appended one gets the arena's.

use crate::falloc::VecPush;
use crate::html::dom::{Document, DomOom, NodeId, NodeType};
use crate::lexbor::adapter::html::{HtmlAttr, HtmlDoc, HtmlNode};

/// The prefix before the first colon of `qname`, when it has one.
fn prefix_of(qname: &[u8]) -> Option<&[u8]> {
    qname.iter().position(|&c| c == b':').map(|i| &qname[..i])
}

/// Copy the whole of `src` into a new arena document.
pub fn copy_document(src: HtmlDoc<'_>) -> Result<Document, DomOom> {
    let mode = u8::try_from(src.compat_mode()).unwrap_or(2);
    let mut doc = Document::new(mode)?;
    copy_names(&mut doc, src.as_node(), NodeId::DOCUMENT)?;
    copy_children(&mut doc, src.as_node(), NodeId::DOCUMENT, true)?;
    Ok(doc)
}

/// Copy the children of `src` (and everything below them) under `dst`, with
/// their source offsets when `offsets` - a copy of a parse keeps them, an
/// import from another document drops them.
pub fn copy_children(
    doc: &mut Document,
    src: HtmlNode<'_>,
    dst: NodeId,
    offsets: bool,
) -> Result<(), DomOom> {
    /* (source parent, its next child to copy, destination parent) */
    let mut stack: Vec<(Option<HtmlNode<'_>>, NodeId)> = Vec::new();
    stack
        .falloc_push((src.first_child(), dst))
        .map_err(|()| DomOom)?;
    while let Some(top) = stack.last_mut() {
        let Some(s) = top.0 else {
            stack.pop();
            continue;
        };
        top.0 = s.next();
        let parent = top.1;
        let made = copy_one(doc, s)?;
        if !offsets {
            doc.forget_source_offset(made);
        }
        doc.append_child(parent, made);
        if let Some(contents) = s.template_content() {
            /* The arena made the contents with the template, as Lexbor
             * does. */
            let frag = doc.template_content(made).ok_or(DomOom)?;
            copy_names(doc, contents, frag)?;
            stack
                .falloc_push((contents.first_child(), frag))
                .map_err(|()| DomOom)?;
        }
        if s.first_child().is_some() {
            stack
                .falloc_push((s.first_child(), made))
                .map_err(|()| DomOom)?;
        }
    }
    Ok(())
}

/// One node, detached, without its children, with Lexbor's name fields.
fn copy_one(doc: &mut Document, s: HtmlNode<'_>) -> Result<NodeId, DomOom> {
    let made = match s.node_type() {
        NodeType::Element => {
            let ns = match s.ns_uri() {
                Some(uri) => doc.intern_ns(uri)?.unwrap_or(0),
                None => 0,
            };
            let el = s.element().ok_or(DomOom)?;
            let tag = doc
                .names
                .tag_append_lower(el.local_name())
                .map_err(|_| DomOom)?;
            let qname = el.qualified_name();
            let prefix = match (s.has_prefix(), prefix_of(qname)) {
                (true, Some(p)) => doc.intern_prefix(p)?.unwrap_or(0),
                _ => 0,
            };
            let written = el.has_written_name().then_some(qname);
            let id = doc.create_element(tag, ns, prefix, written)?;
            let (id_attr, class_attr) = (el.id_attr(), el.class_attr());
            let (mut new_id, mut new_class) = (None, None);
            for a in el.attrs() {
                let made_attr = copy_attr(doc, a)?;
                doc.attr_append(id, made_attr);
                if Some(a) == id_attr {
                    new_id = Some(made_attr);
                }
                if Some(a) == class_attr {
                    new_class = Some(made_attr);
                }
            }
            doc.set_shortcuts(id, new_id, new_class);
            id
        }
        kind => {
            let id = match kind {
                NodeType::Text | NodeType::Comment | NodeType::CDataSection => {
                    doc.create_data(kind, s.data().unwrap_or_default())?
                }
                NodeType::Pi => doc.create_pi(
                    s.pi_target().unwrap_or_default(),
                    s.data().unwrap_or_default(),
                )?,
                NodeType::DocumentType => doc.create_doctype(
                    s.node_name(),
                    s.doctype_public_id().unwrap_or_default(),
                    s.doctype_system_id().unwrap_or_default(),
                )?,
                NodeType::DocumentFragment => doc.create_fragment(None)?,
                _ => return Err(DomOom),
            };
            copy_names(doc, s, id)?;
            id
        }
    };
    if let Some(off) = s.source_offset() {
        doc.set_source_offset(made, off);
    }
    Ok(made)
}

/// Lexbor's name fields of a node that is neither an element nor an
/// attribute: the special tag ids Lexbor gives these (`#text`, `!--`, ...)
/// are all static, and the namespace is whatever creation left there (a
/// template's contents fragment is in the HTML namespace).
fn copy_names(doc: &mut Document, s: HtmlNode<'_>, id: NodeId) -> Result<(), DomOom> {
    let ns = match s.ns_uri() {
        Some(uri) => doc.intern_ns(uri)?.unwrap_or(0),
        None => 0,
    };
    let local = s.tag_id().and_then(|t| t.static_index()).unwrap_or(0);
    doc.set_names(id, local, ns, 0);
    Ok(())
}

/// One attribute, not on any element.
fn copy_attr(doc: &mut Document, a: HtmlAttr<'_>) -> Result<NodeId, DomOom> {
    let local = doc
        .names
        .attr_local_name_append(a.local_name())
        .map_err(|_| DomOom)?;
    /* The namespace Lexbor stores on the node - an element's for an
     * attribute with none of its own - so `attr_own_ns` decides as the
     * adapter does. */
    let ns = match a.node().ns_uri() {
        Some(uri) => doc.intern_ns(uri)?.unwrap_or(0),
        None => 0,
    };
    let qname = a.qualified_name();
    let prefix = match (a.node().has_prefix(), prefix_of(qname)) {
        (true, Some(p)) => doc.intern_prefix(p)?.unwrap_or(0),
        _ => 0,
    };
    let written = (!a.named_by_local()).then_some(qname);
    doc.create_attr(local, ns, prefix, written, a.value())
}
