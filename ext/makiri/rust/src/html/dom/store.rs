//! The byte store: every node's character data, attribute value and
//! doctype string, as [`Span`]s of one append-only `Vec<u8>`.
//!
//! Only this module makes a span or writes the bytes, so what the store
//! promises - a span names bytes no other span names, the dead count is what
//! no span names, a text's reserved room stays its own - is kept here, and
//! the rest of the arena only holds spans and asks for their bytes.
//! Compaction (`compact.rs` decides when) is the store rewriting itself over
//! the spans the arena hands it.

use super::{DomOom, NodeId};
use crate::falloc::{try_vec_with_capacity, Reserve, VecGrow};

/// `[start, start + len)` of the byte store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    start: u32,
    len: u32,
}

impl Span {
    /// The number of bytes the span names.
    #[inline]
    pub(super) fn len(self) -> usize {
        self.len as usize
    }

    /// Whether the span names no bytes.
    #[inline]
    pub(super) fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// How many texts keep room reserved after them at once.
const GROWING: usize = 4;

/// The texts [`ByteStore::append`] moved to the tail of the byte store
/// with room after them: each one's node, where its data ends, and where the
/// room ends. The room is dead bytes no span names; the data grows into it in
/// place. A text is only ever here while its data still ends at `end` - any
/// other change to it stores its data elsewhere - and the store is never
/// written behind a room, so the room stays the text's own until it is used
/// or the store is compacted (which forgets every room).
#[derive(Clone, Copy, Default)]
struct Growing {
    slots: [Option<(NodeId, u32, u32)>; GROWING],
    /// The slot the next text takes.
    next: usize,
}

impl Growing {
    /// Forget every room: the spans are about to move.
    fn clear(&mut self) {
        *self = Growing::default();
    }
}

/// The store itself.
#[derive(Default)]
pub(super) struct ByteStore {
    bytes: Vec<u8>,
    /// Bytes no span names any more: what replacing a node's data left
    /// behind, and the rooms `append` reserves.
    dead: usize,
    /// Texts appended to with room reserved after them.
    growing: Growing,
}

impl ByteStore {
    /// Store `b`.
    pub(super) fn store(&mut self, b: &[u8]) -> Result<Span, DomOom> {
        let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
        let len = u32::try_from(b.len()).map_err(|_| DomOom)?;
        start.checked_add(len).ok_or(DomOom)?;
        self.bytes
            .falloc_extend_from_slice(b)
            .map_err(|()| DomOom)?;
        Ok(Span { start, len })
    }

    /// A second copy of the stored bytes `s`.
    pub(super) fn dup(&mut self, s: Span) -> Result<Span, DomOom> {
        let start = u32::try_from(self.bytes.len()).map_err(|_| DomOom)?;
        start.checked_add(s.len).ok_or(DomOom)?;
        let from = s.start as usize;
        let to = from + s.len as usize;
        if to > self.bytes.len() {
            return Err(DomOom);
        }
        self.bytes
            .falloc_extend_from_within(from..to)
            .map_err(|()| DomOom)?;
        Ok(Span { start, len: s.len })
    }

    /// The bytes `s` names.
    #[inline]
    pub(super) fn get(&self, s: Span) -> &[u8] {
        let start = s.start as usize;
        self.bytes
            .get(start..start + s.len as usize)
            .unwrap_or_default()
    }

    /// `s` is named by nothing any more.
    #[inline]
    pub(super) fn release(&mut self, s: Span) {
        self.dead += s.len as usize;
    }

    /// The bytes held, live and dead.
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.bytes.len()
    }

    /// The bytes allocated.
    #[inline]
    pub(super) fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    /// The bytes no span names.
    #[inline]
    pub(super) fn dead(&self) -> usize {
        self.dead
    }

    /// Rewrite the store with the live bytes only: `spans` hands every span
    /// the arena holds to the function it is given, and is called twice -
    /// to count them, then to move each. Nothing changes when the new store
    /// cannot be allocated.
    pub(super) fn compact(
        &mut self,
        mut spans: impl FnMut(&mut dyn FnMut(&mut Span)),
    ) -> Result<(), DomOom> {
        let mut live = 0usize;
        spans(&mut |s| live += s.len as usize);
        let mut out: Vec<u8> = try_vec_with_capacity(live).ok_or(DomOom)?;
        /* `out` holds every live byte without growing, so nothing below
         * allocates, and every span fits a u32 as it did before. */
        let bytes = &self.bytes;
        /* The plain call, under an allow: `out` was sized to every live byte
         * above, so it cannot grow - and a failure part-way could not be
         * undone, with the spans already moved. */
        #[allow(clippy::disallowed_methods, reason = "sized to the total above")]
        spans(&mut |s| {
            let start = s.start as usize;
            let from = bytes.get(start..start + s.len as usize).unwrap_or_default();
            s.start = out.len() as u32;
            out.extend_from_slice(from);
        });
        self.bytes = out;
        self.dead = 0;
        self.growing.clear();
        Ok(())
    }

    /// The span of the Text node `n`'s data `span` with `b` appended
    /// (`lexbor_str_append` onto its character data): in place when its data ends the byte store, as it
    /// does whenever nothing was stored since it was made, or when it ends at
    /// room reserved for it. Otherwise the data moves to the tail with as
    /// much room again after it, so a text appended to between other stores
    /// (table foster parenting between comments) is copied a number of times
    /// logarithmic in its length - not once per append, which made the store
    /// and the copying quadratic in it.
    pub(super) fn append(&mut self, n: NodeId, span: Span, b: &[u8]) -> Result<Span, DomOom> {
        let len = span.len as usize;
        let end = span.start as usize + len;
        let total = u32::try_from(len + b.len()).map_err(|_| DomOom)?;
        let room = self.growing.slots.iter().position(|s| {
            matches!(*s, Some((node, at, cap)) if node == n && at as usize == end
                && end + b.len() <= cap as usize)
        });
        let new = if end == self.bytes.len() {
            u32::try_from(self.bytes.len() + b.len()).map_err(|_| DomOom)?;
            self.bytes
                .falloc_extend_from_slice(b)
                .map_err(|()| DomOom)?;
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
            /* All three in the room reserved above. */
            self.bytes
                .falloc_extend_from_within(span.start as usize..end)
                .map_err(|()| DomOom)?;
            self.bytes
                .falloc_extend_from_slice(b)
                .map_err(|()| DomOom)?;
            self.bytes
                .falloc_resize(cap as usize, 0)
                .map_err(|()| DomOom)?;
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
        Ok(new)
    }
}
