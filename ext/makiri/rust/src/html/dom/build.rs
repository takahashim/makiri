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

/// How many texts keep room reserved after them at once.
const GROWING: usize = 4;

/// The texts [`Document::append_text`] moved to the tail of the byte store
/// with room after them: each one's node, where its data ends, and where the
/// room ends. The room is dead bytes no span names; the data grows into it in
/// place. A text is only ever here while its data still ends at `end` - any
/// other change to it stores its data elsewhere - and the store is never
/// written behind a room, so the room stays the text's own until it is used
/// or the store is compacted (which forgets every room).
#[derive(Clone, Copy, Default)]
pub struct Growing {
    slots: [Option<(NodeId, u32, u32)>; GROWING],
    /// The slot the next text takes.
    next: usize,
}

impl Growing {
    /// Forget every room: the spans are about to move.
    pub fn clear(&mut self) {
        *self = Growing::default();
    }
}

impl Document {
    /// Append `b` to the data of the Text node `n` (`lexbor_str_append` onto
    /// its character data): in place when its data ends the byte store, as it
    /// does whenever nothing was stored since it was made, or when it ends at
    /// room reserved for it. Otherwise the data moves to the tail with as
    /// much room again after it, so a text appended to between other stores
    /// (table foster parenting between comments) is copied a number of times
    /// logarithmic in its length - not once per append, which made the store
    /// and the copying quadratic in it.
    pub fn append_text(&mut self, n: NodeId, b: &[u8]) -> Result<(), DomOom> {
        let Some(Payload::Data(span)) = self.node(n).map(|x| x.payload) else {
            return Ok(());
        };
        let len = span.len as usize;
        let end = span.start as usize + len;
        let total = u32::try_from(len + b.len()).map_err(|_| DomOom)?;
        let room = self.growing.slots.iter().position(|s| {
            matches!(*s, Some((node, at, cap)) if node == n && at as usize == end
                && end + b.len() <= cap as usize)
        });
        let new = if end == self.bytes.len() {
            u32::try_from(self.bytes.len() + b.len()).map_err(|_| DomOom)?;
            self.bytes.falloc_reserve(b.len()).map_err(|()| DomOom)?;
            self.bytes.extend_from_slice(b);
            Span {
                start: span.start,
                len: total,
            }
        } else if let Some(i) = room {
            /* Into the room: it is dead bytes, and stays dead past the end. */
            if let Some(dst) = self.bytes.get_mut(end..end + b.len()) {
                dst.copy_from_slice(b);
            }
            self.dead -= b.len();
            if let Some((_, at, _)) = &mut self.growing.slots[i] {
                *at = (end + b.len()) as u32;
            }
            Span {
                start: span.start,
                len: total,
            }
        } else {
            /* To the tail, with as much room again after it. */
            let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
            let spare = total as usize;
            let cap = (start as usize)
                .checked_add(total as usize + spare)
                .and_then(|c| u32::try_from(c).ok())
                .ok_or(DomOom)?;
            self.bytes
                .falloc_reserve(total as usize + spare)
                .map_err(|()| DomOom)?;
            self.bytes.extend_from_within(span.start as usize..end);
            self.bytes.extend_from_slice(b);
            self.bytes.resize(cap as usize, 0);
            /* The old data is dead now; its room, if it had one, was counted
             * dead when it was reserved. */
            for s in &mut self.growing.slots {
                if matches!(*s, Some((node, ..)) if node == n) {
                    *s = None;
                }
            }
            self.dead += len + spare;
            let slot = self.growing.next;
            self.growing.slots[slot] = Some((n, start + total, cap));
            self.growing.next = (slot + 1) % GROWING;
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
