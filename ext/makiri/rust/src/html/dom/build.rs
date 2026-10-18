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

/// The room a parse left after text it moved to the tail of the byte store
/// ([`Document::append_text`]): node -> (the data's start when the room was
/// made, the room's end). Owned by one tree builder, so it lives for one
/// parse, where nothing but `append_text` rewrites a text node's data and
/// nothing compacts the store - an entry whose start no longer matches the
/// node's is simply not used.
#[derive(Default)]
pub struct TextRoom(crate::ptr_table::PtrMap<*const (), (u32, u32)>);

impl TextRoom {
    fn key(n: NodeId) -> *const () {
        core::ptr::without_provenance(n.to_word())
    }

    /// Where the room after `n`'s data ends, when its data still starts at
    /// `start`, where the room was made.
    fn end_for(&self, n: NodeId, start: u32) -> Option<usize> {
        let (at, end) = self.0.get(Self::key(n))?;
        (at == start).then_some(end as usize)
    }

    /// Remember room up to `end` after `n`'s data, which starts at `start`.
    /// Out of memory, nothing is remembered: the room only saves moves.
    fn record(&mut self, n: NodeId, start: u32, end: u32) {
        let _ = self.0.set(Self::key(n), (start, end));
    }
}

impl Document {
    /// Append `b` to the data of the Text node `n` (`lexbor_str_append` onto
    /// its character data): in place when its data ends the byte store, as it
    /// does whenever nothing was stored since it was made, or when it fits the
    /// room a previous move left after it; otherwise the data moves to the
    /// tail with `b` after it, and as much room again after that.
    ///
    /// The room is what keeps a parse linear. Text appended alternately to
    /// two nodes - foster-parented text before a `<table>` between its cells,
    /// `<table>x<td>y</td>x<td>...` - moved the first one's whole data on every
    /// append, so the store grew with the square of the input: 700 KB of it
    /// made 2 GB, and a little more ran into the 4 GiB a span can address and
    /// failed the parse. Doubling makes the moves logarithmic, and the room
    /// is dead bytes, which a later compaction drops.
    pub fn append_text(&mut self, n: NodeId, b: &[u8], room: &mut TextRoom) -> Result<(), DomOom> {
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
        } else if let Some(to) = room
            .end_for(n, span.start)
            .filter(|&room_end| end + b.len() <= room_end)
            .and_then(|_| self.bytes.get_mut(end..end + b.len()))
        {
            /* Into the room: those bytes were counted dead when it was made. */
            to.copy_from_slice(b);
            self.dead = self.dead.saturating_sub(b.len());
            Span {
                start: span.start,
                len: total,
            }
        } else {
            let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
            let data_end = start.checked_add(total).ok_or(DomOom)?;
            let want = total.min(u32::MAX - data_end) as usize;
            /* The room is an optimisation: without memory for it, move the
             * data alone, as before. */
            let extra = if self.bytes.falloc_reserve(total as usize + want).is_ok() {
                want
            } else {
                self.bytes
                    .falloc_reserve(total as usize)
                    .map_err(|()| DomOom)?;
                0
            };
            self.bytes.extend_from_within(span.start as usize..end);
            self.bytes.extend_from_slice(b);
            self.bytes.resize(self.bytes.len() + extra, 0); /* reserved above */
            self.dead += len + extra;
            if extra > 0 {
                room.record(n, start, data_end + extra as u32);
            }
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
