//! What the tree builder (`crate::html::tree`) writes that no DOM operation
//! does: text appended to a node it already made, attributes made from an id
//! of the qualified-name table rather than from text, and Lexbor's own
//! `id`/`class` bookkeeping on append.

use super::{AttrData, Document, DomOom, NodeId, NodeType, NsIndex, Payload, PrefixIndex, Span};
use crate::falloc::Reserve;
use crate::html::res::names as res;

/// Where a new attribute's value comes from.
#[derive(Clone, Copy)]
pub enum AttrValue<'a> {
    /// These bytes.
    Bytes(&'a [u8]),
    /// A copy of another attribute's value, in this document.
    CopyOf(NodeId),
}

impl Document {
    /// Append `b` to the data of the Text node `n` (`lexbor_str_append` onto
    /// its character data): in place when its data ends the byte store, as it
    /// does whenever nothing was stored since it was made; otherwise the data
    /// moves to the tail with `b` after it.
    pub fn append_text(&mut self, n: NodeId, b: &[u8]) -> Result<(), DomOom> {
        let Some(Payload::Data(span)) = self.node(n).map(|x| x.payload) else {
            return Ok(());
        };
        let len = span.len as usize;
        let end = span.start as usize + len;
        let total = u32::try_from(len + b.len()).map_err(|_| DomOom)?;
        let new = if end == self.bytes.len() {
            u32::try_from(self.bytes.len() + b.len()).map_err(|_| DomOom)?;
            self.bytes.falloc_reserve(b.len()).map_err(|()| DomOom)?;
            self.bytes.extend_from_slice(b);
            Span {
                start: span.start,
                len: total,
            }
        } else {
            let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
            start.checked_add(total).ok_or(DomOom)?;
            self.bytes
                .falloc_reserve(len + b.len())
                .map_err(|()| DomOom)?;
            self.bytes.extend_from_within(span.start as usize..end);
            self.bytes.extend_from_slice(b);
            Span { start, len: total }
        };
        if let Some(x) = self.node_mut(n) {
            x.payload = Payload::Data(new);
        }
        Ok(())
    }

    /// A new attribute, not on any element, whose qualified name as written
    /// is `written`, an id of [`Names::qualified`](crate::html::names::Names)
    /// (0 for none).
    pub fn create_attr_by_ids(
        &mut self,
        local: usize,
        ns: NsIndex,
        prefix: PrefixIndex,
        written: u32,
        value: AttrValue<'_>,
    ) -> Result<NodeId, DomOom> {
        let value = match value {
            AttrValue::Bytes(b) => self.store(b)?,
            AttrValue::CopyOf(from) => match self.attr(from) {
                Some(a) => {
                    let v = a.value;
                    self.dup(v)?
                }
                None => Span::default(),
            },
        };
        let idx = u32::try_from(self.attrs.len()).map_err(|_| DomOom)?;
        self.attrs.falloc_reserve(1).map_err(|()| DomOom)?;
        let id = self.push(NodeType::Attribute, Payload::Attr(idx))?;
        self.set_names(id, local, ns, prefix);
        self.attrs.push(AttrData {
            written,
            value,
            next: None,
            prev: None,
        });
        Ok(id)
    }

    /// An element's or attribute's written-name id (0 for none).
    pub fn written_id(&self, n: NodeId) -> u32 {
        match self.node(n).map(|x| x.payload) {
            Some(Payload::Element(i)) => self.elements.get(i as usize).map_or(0, |e| e.written),
            Some(Payload::Attr(i)) => self.attrs.get(i as usize).map_or(0, |a| a.written),
            _ => 0,
        }
    }

    /// Set an element's or attribute's written-name id.
    pub fn set_written_id(&mut self, n: NodeId, written: u32) {
        match self.node(n).map(|x| x.payload) {
            Some(Payload::Element(i)) => {
                if let Some(e) = self.elements.get_mut(i as usize) {
                    e.written = written;
                }
            }
            Some(Payload::Attr(i)) => {
                if let Some(a) = self.attrs.get_mut(i as usize) {
                    a.written = written;
                }
            }
            _ => {}
        }
    }

    /// `lxb_dom_element_attr_append`: link `attr` last on `el`, and when its
    /// local name is `id` or `class` (whatever its namespace) make it the
    /// element's shortcut, taking off the attribute that held it.
    pub fn attr_append_lexbor(&mut self, el: NodeId, attr: NodeId) {
        let local = self.local(attr) as usize;
        if local == res::ATTR_ID {
            if let Some(old) = self.id_attr(el) {
                self.attr_remove(old);
            }
            if let Some(e) = self.element_mut(el) {
                e.id_attr = Some(attr);
            }
        } else if local == res::ATTR_CLASS {
            if let Some(old) = self.class_attr(el) {
                self.attr_remove(old);
            }
            if let Some(e) = self.element_mut(el) {
                e.class_attr = Some(attr);
            }
        }
        self.attr_link(el, attr);
    }
}
