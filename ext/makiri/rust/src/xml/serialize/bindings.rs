//! The namespace bindings in scope while a serializer walks a tree: one stack,
//! innermost last, with a step budget so resolution cannot be made to run
//! unboundedly long. Both writers keep their scope here - the XML writer to
//! plan prefixes, the Canonical XML writer to check that the declarations it
//! renders give each name the namespace it has.

#![forbid(unsafe_code)]

use super::out::{room, W};
use super::Failure;
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

/// Every binding in scope at the current element, innermost last, with an
/// index from prefix to its innermost binding.
///
/// One element's entries are its own xmlns declarations plus at most one
/// declaration the planner synthesized for the element's own name; they are
/// pushed on entry and truncated away on exit. Within one element the order is
/// immaterial: a prefix is declared at most once per element (the parser rejects
/// a duplicate and the DOM replaces it), and an invented prefix is chosen
/// unbound, so no two entries from the same element share a prefix.
///
/// The index makes a lookup cost what hashing the prefix costs, where a scan
/// down the stack cost its depth: with thousands of declarations in scope,
/// every name of every element paid that depth, and both writers ran out of
/// their step budget on legitimate documents. Each entry remembers the binding
/// of its prefix it SHADOWS, so leaving an element restores the outer one.
pub(super) struct Bindings<'d> {
    stack: Vec<Entry<'d>>,
    /// Open addressing over the prefixes in scope: each slot is [`EMPTY`],
    /// [`GONE`] (a prefix whose last binding was truncated away), or the stack
    /// index of that prefix's innermost binding.
    slots: Vec<u32>,
    /// Slots that are not [`EMPTY`] - live ones and [`GONE`] ones - which is
    /// what decides when the table is rebuilt.
    filled: usize,
    /// Lookups taken so far, against [`NS_STEP_MAX`]. Once past it every
    /// lookup fails with [`Failure::NamespaceBudget`] - the count only grows,
    /// so no separate flag is needed to keep it failing.
    steps: u64,
}

struct Entry<'d> {
    prefix: Prefix<'d>,
    uri: &'d [u8],
    /// The stack index of the binding of the same prefix this one hides, or
    /// [`EMPTY`] when it hides none.
    shadows: u32,
}

const EMPTY: u32 = u32::MAX;
const GONE: u32 = u32::MAX - 1;

/// FNV-1a: prefixes are short, and the table only needs them spread.
fn hash(prefix: &[u8]) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in prefix {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h as usize
}

/// Where a probe for a prefix ended.
enum Probe {
    /// The slot holding its innermost binding.
    Found(usize),
    /// Not in scope; the slot a new binding of it would take.
    Absent(usize),
}

impl<'d> Bindings<'d> {
    pub(super) fn new() -> Self {
        Bindings {
            stack: Vec::new(),
            slots: Vec::new(),
            filled: 0,
            steps: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.stack.len()
    }

    /// The slot for `prefix`. The table always has an [`EMPTY`] slot (it is
    /// rebuilt at half full), so the probe ends.
    fn probe(&self, prefix: &[u8]) -> Probe {
        let mask = self.slots.len() - 1;
        let mut i = hash(prefix) & mask;
        let mut free = None;
        loop {
            match self.slots[i] {
                EMPTY => return Probe::Absent(free.unwrap_or(i)),
                GONE => {
                    free.get_or_insert(i);
                }
                at => {
                    if self.stack[at as usize].prefix.bytes() == prefix {
                        return Probe::Found(i);
                    }
                }
            }
            i = (i + 1) & mask;
        }
    }

    /// Room for one more prefix: rebuilt from the stack, larger, once the
    /// filled slots would pass half.
    fn make_room(&mut self) -> W {
        if !self.slots.is_empty() && (self.filled + 1) * 2 <= self.slots.len() {
            return Ok(());
        }
        let size = (self.stack.len() + 1)
            .checked_mul(4)
            .map(|n| n.next_power_of_two().max(16))
            .ok_or(Failure::Output)?;
        let mut slots: Vec<u32> = Vec::new();
        room(slots.falloc_reserve_exact(size))?;
        slots.resize(size, EMPTY);
        let old = core::mem::replace(&mut self.slots, slots);
        self.filled = 0;
        /* Outermost first, so each prefix ends up at its innermost binding. */
        for at in 0..self.stack.len() {
            match self.probe(self.stack[at].prefix.bytes()) {
                Probe::Found(i) => self.slots[i] = at as u32,
                Probe::Absent(i) => {
                    self.slots[i] = at as u32;
                    self.filled += 1;
                }
            }
        }
        drop(old);
        Ok(())
    }

    pub(super) fn truncate(&mut self, base: usize) {
        while self.stack.len() > base {
            let top = self.stack.len() - 1;
            let shadows = self.stack[top].shadows;
            if let Probe::Found(i) = self.probe(self.stack[top].prefix.bytes()) {
                self.slots[i] = if shadows == EMPTY { GONE } else { shadows };
            }
            self.stack.pop();
        }
    }

    pub(super) fn push(&mut self, prefix: Prefix<'d>, uri: &'d [u8]) -> W {
        let at = u32::try_from(self.stack.len())
            .ok()
            .filter(|&n| n < GONE)
            .ok_or(Failure::Output)?;
        room(self.stack.falloc_reserve(1))?;
        self.make_room()?;
        let shadows = match self.probe(prefix.bytes()) {
            Probe::Found(i) => core::mem::replace(&mut self.slots[i], at),
            Probe::Absent(i) => {
                if self.slots[i] == EMPTY {
                    self.filled += 1;
                }
                self.slots[i] = at;
                EMPTY
            }
        };
        self.stack.push(Entry {
            prefix,
            uri,
            shadows,
        });
        Ok(())
    }

    /// Every prefix in scope with its innermost binding, in no order: what
    /// the index holds, read without a lookup per prefix.
    pub(super) fn innermost(&self) -> impl Iterator<Item = (&Prefix<'d>, &'d [u8])> + '_ {
        self.slots
            .iter()
            .filter(|&&at| at != EMPTY && at != GONE)
            .map(|&at| {
                let e = &self.stack[at as usize];
                (&e.prefix, e.uri)
            })
    }

    /// The innermost binding for `prefix`, or `Ok(None)` when it is unbound.
    /// [`Failure::NamespaceBudget`] once the step budget is spent - raised
    /// here, where it runs out, so no caller can mistake an exhausted lookup
    /// for an unbound prefix and emit a declaration it did not verify.
    pub(super) fn lookup(&mut self, prefix: &[u8]) -> Result<Option<&'d [u8]>, Failure> {
        self.steps += 1;
        if self.steps > NS_STEP_MAX {
            return Err(Failure::NamespaceBudget);
        }
        if self.slots.is_empty() {
            return Ok(None);
        }
        Ok(match self.probe(prefix) {
            Probe::Found(i) => Some(self.stack[self.slots[i] as usize].uri),
            Probe::Absent(_) => None,
        })
    }

    /// What `prefix` means in this scope (Namespaces in XML §3, §6.2): `xml`
    /// its fixed URI, bound everywhere and never declared; an undeclared
    /// default no namespace (`""`); an undeclared prefix nothing, `Ok(None)`,
    /// which no name may carry. `Err` once the step budget is spent.
    pub(super) fn resolve(&mut self, prefix: &[u8]) -> Result<Option<&'d [u8]>, Failure> {
        if prefix == b"xml" {
            return Ok(Some(crate::xml::XML_NS_URI));
        }
        Ok(match self.lookup(prefix)? {
            Some(uri) => Some(uri),
            None if prefix.is_empty() => Some(b""),
            None => None,
        })
    }

    /// Whether `prefix` means `uri` here, by [`resolve`](Self::resolve) - and
    /// `xml` always, whatever `uri` says: a name not yet decided carries no
    /// URI, and the mutators refuse `xml` with any namespace but its own, so
    /// `xml` never needs a declaration.
    pub(super) fn bound_to(&mut self, prefix: &[u8], uri: &[u8]) -> Result<bool, Failure> {
        Ok(prefix == b"xml" || self.resolve(prefix)? == Some(uri))
    }

    pub(super) fn is_bound(&mut self, prefix: &[u8]) -> Result<bool, Failure> {
        Ok(prefix == b"xml" || self.lookup(prefix)?.is_some())
    }
}
