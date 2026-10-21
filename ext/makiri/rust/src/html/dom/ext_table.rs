//! The namespace and prefix tables: Lexbor's static entries first, then the
//! document's own, numbered from the static table's length (`NS__LAST_ENTRY`
//! for both) - the same scheme for both, which `Document` used to write out
//! twice.

use super::DomOom;
use crate::falloc::VecPush;
use crate::html::res::names as res;

/// One table of byte strings over a static prefix of Lexbor's.
pub(super) struct ExtTable {
    /// Lexbor's static entries by id; an empty one is no entry.
    statics: &'static [&'static [u8]],
    /// The document's own, numbered from `statics.len()`.
    own: Vec<Vec<u8>>,
    /// The bytes of `own`'s entries: its growth in O(1).
    bytes: usize,
}

impl ExtTable {
    pub(super) const fn new(statics: &'static [&'static [u8]]) -> ExtTable {
        ExtTable {
            statics,
            own: Vec::new(),
            bytes: 0,
        }
    }

    /// The id of `text`, matched byte for byte; `None` when the table has
    /// none by that text (or for an empty one). The static search starts
    /// at the HTML entry, as Lexbor's does.
    pub(super) fn lookup(&self, text: &[u8]) -> Option<u32> {
        if text.is_empty() {
            return None;
        }
        if let Some(i) = self
            .statics
            .iter()
            .skip(res::NS_HTML)
            .position(|l| *l == text)
        {
            return u32::try_from(i + res::NS_HTML).ok();
        }
        self.own
            .iter()
            .position(|l| l == text)
            .and_then(|i| u32::try_from(self.statics.len() + i).ok())
    }

    /// [`lookup`](Self::lookup), appending `text` when new.
    pub(super) fn intern(&mut self, text: &[u8]) -> Result<Option<u32>, DomOom> {
        if text.is_empty() {
            return Ok(None);
        }
        if let Some(i) = self.lookup(text) {
            return Ok(Some(i));
        }
        let owned = crate::falloc::try_to_vec(text).ok_or(DomOom)?;
        self.own.falloc_push(owned).map_err(|()| DomOom)?;
        self.bytes += text.len();
        Ok(u32::try_from(self.statics.len() + self.own.len() - 1).ok())
    }

    /// An id's text; `None` for 0, an empty static entry or an unknown id.
    pub(super) fn get(&self, id: u32) -> Option<&[u8]> {
        let i = id as usize;
        if i == 0 {
            return None;
        }
        match self.statics.get(i) {
            Some(l) => (!l.is_empty()).then_some(*l),
            None => self
                .own
                .get(i.checked_sub(self.statics.len())?)
                .map(Vec::as_slice),
        }
    }

    /// What the table owns, for the GC report.
    pub(super) fn owned_bytes(&self) -> usize {
        self.own.iter().map(Vec::capacity).sum::<usize>()
            + self.own.capacity() * core::mem::size_of::<Vec<u8>>()
    }

    /// The bytes of the document's own entries, in O(1).
    pub(super) fn stored_bytes(&self) -> usize {
        self.bytes
    }

    /// The longest of the document's own entries.
    pub(super) fn longest_own(&self) -> usize {
        self.own.iter().map(Vec::len).max().unwrap_or(0)
    }
}
