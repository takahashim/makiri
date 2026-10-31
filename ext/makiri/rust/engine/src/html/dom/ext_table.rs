//! The namespace and prefix tables: Lexbor's static entries first, then the
//! document's own, numbered from the static table's length (`NS__LAST_ENTRY`
//! for both) - the same scheme for both, which `Document` used to write out
//! twice.

use super::DomOom;
use crate::html::names::ExactNames;
use crate::html::res::names as res;

/// One table of byte strings over a static prefix of Lexbor's.
pub(super) struct ExtTable {
    /// Lexbor's static entries by id; an empty one is no entry.
    statics: &'static [&'static [u8]],
    /// The document's own, numbered from `statics.len()` and hashed, so a
    /// document with many namespaces does not look each up in a scan.
    own: ExactNames,
}

impl ExtTable {
    pub(super) fn new(statics: &'static [&'static [u8]]) -> ExtTable {
        ExtTable {
            statics,
            own: ExactNames::default(),
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
            .find(text)
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
        let i = self.own.intern(text).map_err(|_| DomOom)?;
        Ok(u32::try_from(self.statics.len() + i).ok())
    }

    /// An id's text; `None` for 0, an empty static entry or an unknown id.
    pub(super) fn get(&self, id: u32) -> Option<&[u8]> {
        let i = id as usize;
        if i == 0 {
            return None;
        }
        match self.statics.get(i) {
            Some(l) => (!l.is_empty()).then_some(*l),
            None => self.own.get(i.checked_sub(self.statics.len())?),
        }
    }

    /// What the table owns, for the GC report.
    pub(super) fn owned_bytes(&self) -> usize {
        self.own.owned_bytes()
    }

    /// The bytes of the document's own entries, in O(1).
    pub(super) fn stored_bytes(&self) -> usize {
        self.own.stored_bytes()
    }

    /// The longest of the document's own entries.
    pub(super) fn longest_own(&self) -> usize {
        self.own.longest()
    }
}
