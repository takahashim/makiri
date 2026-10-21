//! Copying nodes: within one document (DOM `cloneNode`) and from another
//! (`importNode`, and every insert of a node from another document).
//!
//! The result is what the Lexbor adapter's copy hands out once Makiri's
//! fix-ups have run (`lexbor::fragment::import_with_fixup`): every node keeps
//! its kind, its name fields, the name it was written with, its namespace as
//! written, its attributes in order with the `id`/`class` shortcuts on the
//! copies of the source's, its data, and - for a deep copy - its children and
//! every `<template>`'s contents. A copy within the document keeps the source
//! offsets; one from another document drops them, since they index that
//! document's source.
//!
//! The walk is iterative, so no tree depth grows the native stack. A failed
//! allocation leaves a partial copy that nothing links to, as the arena keeps
//! every node it ever made.

use super::{
    Document, DomOom, ElementData, Node, NodeId, NodeName, NodeType, Payload, ShortcutRule, Span,
    HTML_NS,
};
use crate::falloc::VecPush;
use crate::html::res::names as res;

/// Where a copy reads from.
#[derive(Clone, Copy)]
enum Source<'s> {
    /// The document being written.
    Same,
    /// Another document.
    Other(&'s Document),
}

impl Document {
    /// A copy of the node `n` of this document, detached; with its subtree
    /// when `deep`.
    pub fn copy_within(&mut self, n: NodeId, deep: bool) -> Result<NodeId, DomOom> {
        self.copy_tree(Source::Same, n, deep)
    }

    /// A copy of the node `n` of `src`, another document, detached; with its
    /// subtree when `deep`.
    pub fn copy_from(&mut self, src: &Document, n: NodeId, deep: bool) -> Result<NodeId, DomOom> {
        if core::ptr::eq(self, src) {
            return self.copy_within(n, deep);
        }
        self.copy_tree(Source::Other(src), n, deep)
    }

    fn src<'a>(&'a self, from: Source<'a>) -> &'a Document {
        match from {
            Source::Same => self,
            Source::Other(d) => d,
        }
    }

    fn copy_tree(&mut self, from: Source<'_>, root: NodeId, deep: bool) -> Result<NodeId, DomOom> {
        let top = self.copy_one(from, root)?;
        if !deep {
            return Ok(top);
        }
        /* A DocumentFragment's copy is made afresh and given copies of its
         * children, which is what the walk below does for any node. */
        let mut stack: Vec<(Option<NodeId>, NodeId)> = Vec::new();
        let first = self.src(from).first_child(root);
        stack.falloc_push((first, top)).map_err(|()| DomOom)?;
        if let Some(c) = self.src(from).template_of(root) {
            let to = self.template_content(top).ok_or(DomOom)?;
            let first = self.src(from).first_child(c);
            stack.falloc_push((first, to)).map_err(|()| DomOom)?;
        }
        while let Some(top_entry) = stack.last_mut() {
            let Some(s) = top_entry.0 else {
                stack.pop();
                continue;
            };
            let parent = top_entry.1;
            top_entry.0 = self.src(from).next(s);
            let made = self.copy_one(from, s)?;
            self.append_child(parent, made);
            if let Some(c) = self.src(from).template_of(s) {
                let to = self.template_content(made).ok_or(DomOom)?;
                let first = self.src(from).first_child(c);
                stack.falloc_push((first, to)).map_err(|()| DomOom)?;
            }
            let first = self.src(from).first_child(s);
            if first.is_some() {
                stack.falloc_push((first, made)).map_err(|()| DomOom)?;
            }
        }
        Ok(top)
    }

    /// An HTML `<template>`'s contents, the only contents a copy carries.
    fn template_of(&self, n: NodeId) -> Option<NodeId> {
        let node = self.node(n)?;
        if node.kind == NodeType::Element
            && node.local as usize == res::TAG_TEMPLATE
            && node.ns == HTML_NS
        {
            self.template_content(n)
        } else {
            None
        }
    }

    /// One node, detached and without children: an element with copies of
    /// its attributes (and, for a template, empty contents), anything else
    /// with its data.
    fn copy_one(&mut self, from: Source<'_>, n: NodeId) -> Result<NodeId, DomOom> {
        let node: Node = *self.src(from).node(n).ok_or(DomOom)?;
        let (local, ns, prefix) = self.names_from(from, &node)?;
        let made = match node.payload {
            Payload::Element(i) => {
                let e: ElementData = *self.src(from).elements.get(i as usize).ok_or(DomOom)?;
                let written = self.written_from(from, e.written)?;
                let id = self.push_element(local as usize, ns, prefix, written)?;
                /* The copy's shortcuts point where the source's do. */
                let mut a = e.first_attr;
                while let Some(src_attr) = a {
                    let copy = self.copy_one(from, src_attr)?;
                    let rule = ShortcutRule::take(e.shortcut_of(src_attr));
                    self.link_attribute(id, copy, rule);
                    a = self.src(from).next_attr(src_attr);
                }
                id
            }
            Payload::Attr(i) => {
                let a = *self.src(from).attrs.get(i as usize).ok_or(DomOom)?;
                let written = self.written_from(from, a.written)?;
                let value = self.bytes_from(from, a.value)?;
                let at = self.push_attr(local as usize, ns, prefix, written, value)?;
                /* The copy is in the namespace its source is in (D1). */
                let own = self.src(from).attr_own_ns_kept(n);
                self.set_attr_own_ns(at, own);
                at
            }
            Payload::Data(span) => {
                let span = self.bytes_from(from, span)?;
                let id = self.push(node.kind, Payload::Data(span))?;
                self.set_names(id, local as usize, ns, prefix);
                id
            }
            Payload::Pi { target, data } => {
                let target = self.bytes_from(from, target)?;
                let data = self.bytes_from(from, data)?;
                let id = self.push(NodeType::Pi, Payload::Pi { target, data })?;
                self.set_names(id, local as usize, ns, prefix);
                id
            }
            Payload::Doctype(i) => {
                let d = *self.src(from).doctypes.get(i as usize).ok_or(DomOom)?;
                let dt = super::DoctypeData {
                    name: self.bytes_from(from, d.name)?,
                    public_id: self.bytes_from(from, d.public_id)?,
                    system_id: self.bytes_from(from, d.system_id)?,
                };
                let names = NodeName {
                    local: local as usize,
                    ns,
                    prefix,
                };
                self.push_doctype(names, dt)?
            }
            /* A fragment's copy is a plain new one, as the adapter makes it. */
            Payload::Fragment { .. } => self.create_fragment(None)?,
            /* A document is never copied: `clone_node` and `import_node`
             * refuse one first, and `#dup` parses afresh. Refused here too,
             * so a caller that forgot fails rather than copies. */
            Payload::Document => {
                debug_assert!(false, "a copy of a document node");
                return Err(DomOom);
            }
        };
        if matches!(from, Source::Same) && node.source != 0 {
            if let Some(x) = self.node_mut(made) {
                x.source = node.source;
            }
        }
        Ok(made)
    }

    /// The bytes `s` of the source, stored here.
    fn bytes_from(&mut self, from: Source<'_>, s: Span) -> Result<Span, DomOom> {
        match from {
            Source::Same => self.dup(s),
            Source::Other(d) => self.store(d.bytes_of(s)),
        }
    }

    /// A written-name id of the source, as an id here.
    fn written_from(&mut self, from: Source<'_>, id: u32) -> Result<u32, DomOom> {
        match from {
            Source::Same => Ok(id),
            Source::Other(d) => match d.names.qualified(id) {
                Some(w) => self.names.qualified_append(w).map_err(|_| DomOom),
                None => Ok(0),
            },
        }
    }

    /// The name fields of the source node `node`, as ids here: a name past
    /// the static ones is interned again by its text (a tag with upper case
    /// as written, as Lexbor's copy appends it), a namespace by its URI as
    /// written.
    fn names_from(&mut self, from: Source<'_>, node: &Node) -> Result<(u32, u32, u32), DomOom> {
        let d = match from {
            Source::Same => return Ok((node.local, node.ns, node.prefix)),
            Source::Other(d) => d,
        };
        let local = match node.kind {
            NodeType::Attribute => {
                if (node.local as usize) < res::ATTR__LAST_ENTRY {
                    node.local
                } else {
                    let name = d.names.attr_name(node.local as usize).ok_or(DomOom)?;
                    let id = self
                        .names
                        .attr_local_name_append(name)
                        .map_err(|_| DomOom)?;
                    u32::try_from(id).map_err(|_| DomOom)?
                }
            }
            _ => {
                if (node.local as usize) < res::TAG__LAST_ENTRY {
                    node.local
                } else {
                    let name = d.names.tag_name(node.local as usize).ok_or(DomOom)?;
                    let id = if super::has_ascii_uppercase(name) {
                        self.names.tag_append_raw(name)
                    } else {
                        self.names.tag_append_lower(name)
                    }
                    .map_err(|_| DomOom)?;
                    u32::try_from(id).map_err(|_| DomOom)?
                }
            }
        };
        let ns = if (node.ns as usize) < res::NS__LAST_ENTRY {
            node.ns
        } else {
            let uri = d.ns_uri(node.ns).ok_or(DomOom)?;
            self.intern_ns(uri)?.unwrap_or(0)
        };
        let prefix = if (node.prefix as usize) < res::NS__LAST_ENTRY {
            node.prefix
        } else {
            let text = d.prefix_text(node.prefix).ok_or(DomOom)?;
            self.intern_prefix(text)?.unwrap_or(0)
        };
        Ok((local, ns, prefix))
    }
}
