//! Compacting the byte store (`docs/rewrite_parser.ja.md`, "byte アリーナは
//! 追記型から始める").
//!
//! The store is append-only: replacing a node's data (`content=`, an
//! attribute set) stores the new bytes and leaves the old ones behind, where
//! Lexbor gives them back to its pool. A document edited the same way over
//! and over would grow without bound - to a refusal at 4 GiB, the most a span
//! can address - so the store is rewritten with only the bytes some span
//! names once enough of it is dead.
//!
//! - **What is live**: every span every node, attribute and doctype holds,
//!   detached or not - a removed node is still readable through a Ruby
//!   wrapper. Spans are never shared (a copy always takes its own), so each
//!   is copied once.
//! - **When**: only where no span is held outside the arena - at the start of
//!   an edit, after the indexes are dropped (`HtmlParsed::edit`; the text
//!   index keeps spans). A factory's lend (`edit_keeping_indexes`) does not
//!   compact.
//! - **Atomically**: the new store is allocated at its exact size first; if
//!   that fails nothing has changed and the edit goes on with the old store
//!   (the bytes are still all there - compacting is never needed for an
//!   answer), and nothing after it can fail.
//! - **Amortised**: it runs when the dead bytes exceed both the live bytes
//!   and [`COMPACT_PER_HOLDER`] per span holder, so the walk over every node
//!   is paid for by bytes stored since the last one.

use super::{Document, DomOom, Payload, Span};
use crate::falloc::try_vec_with_capacity;

/// Dead bytes, per node and attribute, that make a compaction worth its
/// walk.
const COMPACT_PER_HOLDER: usize = 8;

impl Document {
    /// Bytes of the store no span names.
    #[inline]
    pub fn dead_bytes(&self) -> usize {
        self.dead
    }

    /// Whether [`compact`](Self::compact) pays for itself now.
    pub fn compaction_due(&self) -> bool {
        let live = self.bytes.len().saturating_sub(self.dead);
        let holders = self.nodes.len() + self.attrs.len();
        self.dead > live && self.dead > holders.saturating_mul(COMPACT_PER_HOLDER)
    }

    /// Rewrite the byte store with the live bytes only. Nothing changes when
    /// the new store cannot be allocated.
    pub fn compact(&mut self) -> Result<(), DomOom> {
        let mut live = 0usize;
        self.each_span(|s| live += s.len as usize);
        let mut out: Vec<u8> = try_vec_with_capacity(live).ok_or(DomOom)?;
        /* `out` holds every live byte without growing, so nothing below
         * allocates, and every span fits a u32 as it did before. */
        let bytes = &self.bytes;
        let mut copy = |s: &mut Span| {
            let start = s.start as usize;
            let from = bytes.get(start..start + s.len as usize).unwrap_or_default();
            s.start = out.len() as u32;
            out.extend_from_slice(from);
        };
        for n in &mut self.nodes {
            match &mut n.payload {
                Payload::Data(s) => copy(s),
                Payload::Pi { target, data } => {
                    copy(target);
                    copy(data);
                }
                _ => {}
            }
        }
        for a in &mut self.attrs {
            copy(&mut a.value);
        }
        for d in &mut self.doctypes {
            copy(&mut d.name);
            copy(&mut d.public_id);
            copy(&mut d.system_id);
        }
        self.bytes = out;
        self.dead = 0;
        Ok(())
    }

    /// Every span the arena holds.
    fn each_span(&self, mut f: impl FnMut(Span)) {
        for n in &self.nodes {
            match n.payload {
                Payload::Data(s) => f(s),
                Payload::Pi { target, data } => {
                    f(target);
                    f(data);
                }
                _ => {}
            }
        }
        for a in &self.attrs {
            f(a.value);
        }
        for d in &self.doctypes {
            f(d.name);
            f(d.public_id);
            f(d.system_id);
        }
    }
}
