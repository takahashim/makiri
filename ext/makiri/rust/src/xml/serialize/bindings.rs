//! The namespace bindings in scope while a serializer walks a tree: one stack,
//! innermost last, with a step budget so resolution cannot be made to run
//! unboundedly long. Both writers keep their scope here - the XML writer to
//! plan prefixes, the Canonical XML writer to check that the declarations it
//! renders give each name the namespace it has.

#![forbid(unsafe_code)]

use super::out::W;
use crate::falloc::Reserve;

/// The total prefix-resolution steps one serialization may take. Generous - an
/// ordinary document uses a handful - but finite, so namespace planning cannot
/// be made to run unboundedly long by nesting and prefix count alone.
pub(super) const NS_STEP_MAX: u64 = 64 * 1024 * 1024;

pub(super) const PREFIX_CAP: usize = 8;

/// A namespace prefix: borrowed from the arena when the document supplied it,
/// owned inline when the serializer invented it.
#[derive(Clone)]
pub(super) enum Prefix<'d> {
    Own(&'d [u8]),
    Invented([u8; PREFIX_CAP], usize),
}

impl Prefix<'_> {
    pub(super) fn bytes(&self) -> &[u8] {
        match self {
            Prefix::Own(s) => s,
            Prefix::Invented(b, n) => &b[..*n],
        }
    }
    pub(super) fn is_invented(&self) -> bool {
        matches!(self, Prefix::Invented(..))
    }
}

/// Every binding in scope at the current element, innermost last.
///
/// One element's entries are its own xmlns declarations plus at most one
/// declaration the planner synthesized for the element's own name; they are
/// pushed on entry and truncated away on exit. Within one element the order is
/// immaterial: a prefix is declared at most once per element (the parser rejects
/// a duplicate and the DOM replaces it), and an invented prefix is chosen
/// unbound, so no two entries from the same element share a prefix.
pub(super) struct Bindings<'d> {
    stack: Vec<(Prefix<'d>, &'d [u8])>,
    steps: u64,
    /// Latched once the step budget is spent. A lookup then answers `None`,
    /// which every caller turns into a refusal, so an exhausted planner can
    /// never emit a declaration it did not verify.
    pub(super) exhausted: bool,
    /// Latched when a name's prefix is bound to nothing: a declaration for it
    /// would be `xmlns:p=""`, which Namespaces in XML forbids, and leaving it
    /// out writes an unbound prefix. Either way no well-formed output exists,
    /// so the writer refuses with [`super::Failure::UnboundPrefix`].
    pub(super) unbound: bool,
}

impl<'d> Bindings<'d> {
    pub(super) fn new() -> Self {
        Bindings {
            stack: Vec::new(),
            steps: 0,
            exhausted: false,
            unbound: false,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.stack.len()
    }

    pub(super) fn truncate(&mut self, base: usize) {
        self.stack.truncate(base);
    }

    pub(super) fn push(&mut self, prefix: Prefix<'d>, uri: &'d [u8]) -> W {
        self.stack.falloc_reserve(1).map_err(|_| ())?;
        self.stack.push((prefix, uri));
        Ok(())
    }

    /// The innermost binding for `prefix`, or None when it is unbound - or when
    /// the step budget ran out, which [`Bindings::exhausted`] then reports.
    pub(super) fn lookup(&mut self, prefix: &[u8]) -> Option<&'d [u8]> {
        for (p, uri) in self.stack.iter().rev() {
            self.steps += 1;
            if self.steps > NS_STEP_MAX {
                self.exhausted = true;
                return None;
            }
            if p.bytes() == prefix {
                return Some(uri);
            }
        }
        None
    }

    /// `xml` is bound everywhere and never declared (Namespaces in XML §3).
    pub(super) fn bound_to(&mut self, prefix: &[u8], uri: &[u8]) -> bool {
        if prefix == b"xml" {
            return true;
        }
        match self.lookup(prefix) {
            None => prefix.is_empty() && uri.is_empty(),
            Some(got) => got == uri,
        }
    }

    pub(super) fn is_bound(&mut self, prefix: &[u8]) -> bool {
        prefix == b"xml" || self.lookup(prefix).is_some()
    }
}
