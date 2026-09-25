//! The namespace bindings in scope while the tree is built, as one stack with a
//! frame per open element.
//!
//! A type rather than two `Vec` fields on the parser plus a `truncate(base)`
//! convention at every exit: the frame discipline - push a base when an element
//! opens, cut back to it when it closes - is the thing that can go wrong, and
//! [`Scope::enter`] / [`Scope::leave`] are the only way to reach it.
//! `serialize::xml`'s `Bindings` is the same shape on the output side.
//!
//! A binding OWNS its prefix bytes (the input slice they came from outlives the
//! parse, but a fragment seeded from a live document's declarations reads its
//! prefixes out of the ARENA, which a later `store` can reallocate) and refers to
//! its URI by [`Span`], because the URI is already in the document.

#![forbid(unsafe_code)]

use crate::falloc::{Reserve, VecPush};
use crate::xml::{Span, Status, MAX_NS};

/// One binding: prefix ("" = the default namespace) -> byte-store span.
struct Binding {
    pfx: Vec<u8>,
    uri: Span,
}

/// Where one element's bindings start, handed out by [`Scope::enter`] and given
/// back to [`Scope::leave`]. Opaque, so it cannot be confused with an index into
/// anything else.
#[derive(Clone, Copy)]
pub(super) struct Frame(usize);

#[derive(Default)]
pub(super) struct Scope {
    binds: Vec<Binding>,
}

/// Why a binding could not be added: the scope's own cap, or memory.
pub(super) enum ScopeFull {
    Limit,
    Oom,
}

impl From<ScopeFull> for Status {
    #[inline]
    fn from(e: ScopeFull) -> Self {
        match e {
            ScopeFull::Limit => Status::Limit,
            ScopeFull::Oom => Status::Oom,
        }
    }
}

impl Scope {
    /// Open an element: remember where its bindings begin.
    pub(super) fn enter(&self) -> Frame {
        Frame(self.binds.len())
    }

    /// Close an element: drop everything it declared.
    pub(super) fn leave(&mut self, frame: Frame) {
        self.binds.truncate(frame.0);
    }

    /// Close an element whose frame is missing, which cannot happen: a frame is
    /// pushed with the element and popped with it, under one guard.
    ///
    /// It has a body anyway, and the body throws EVERY binding away. That is what
    /// the code this was extracted from did (`frame.pop().unwrap_or(0)`), and the
    /// extraction quietly turned it into "do nothing" - the wrong direction for a
    /// fail-closed parser, where a scope stack that has lost track of its frames
    /// must not keep answering lookups from it.
    pub(super) fn leave_without_frame(&mut self) {
        debug_assert!(false, "an element closed with no namespace frame");
        self.binds.clear();
    }

    /// Bind `pfx` to `uri` for the current element and everything under it.
    pub(super) fn bind(&mut self, pfx: &[u8], uri: Span) -> Result<(), ScopeFull> {
        if self.binds.len() + 1 > MAX_NS {
            return Err(ScopeFull::Limit);
        }
        let mut owned: Vec<u8> = Vec::new();
        owned
            .falloc_reserve_exact(pfx.len())
            .map_err(|()| ScopeFull::Oom)?;
        owned.extend_from_slice(pfx);
        self.binds
            .falloc_push(Binding { pfx: owned, uri })
            .map_err(|()| ScopeFull::Oom)
    }

    /// The innermost binding for `pfx`, or None when it is unbound.
    ///
    /// `xml` is NOT handled here: it is bound without a declaration, but to a
    /// URI the DOCUMENT owns, so the caller that has the document answers it.
    pub(super) fn lookup(&self, pfx: &[u8]) -> Option<Span> {
        self.binds
            .iter()
            .rev()
            .find(|b| b.pfx == pfx)
            .map(|b| b.uri)
    }
}
