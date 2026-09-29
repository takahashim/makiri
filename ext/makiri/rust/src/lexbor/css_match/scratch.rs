//! The tables and stacks a query borrows and gives back ([`Scratch`]), and
//! the table type they are borrowed as ([`Table`]) - Lexbor's entry and
//! nested-state pools, kept across calls in `lxb_selectors_t`, as `falloc`
//! vectors.

use crate::falloc::{OomResult, VecPush};
use crate::lexbor::adapter::html::HtmlNode;
use crate::lexbor::css_parser::Lists;

use super::compile::{Chain, Compound, Step};
use super::positions::Positions;
use super::query::{HasCursor, Task};
use super::simple::Name;
use super::MatchFailure;

/// One of a query's tables: a `falloc` vector, borrowed from a [`Scratch`]
/// for the query and handed back after it, so a warm call reuses the last
/// one's allocation instead of making its own - Lexbor's pools, reset
/// between calls, do the same. (It used to keep its first few items inline,
/// which spared the allocation only for small selectors and cost every
/// call the initialisation and the moves of those inline arrays.)
pub(super) struct Table<T>(Vec<T>);

impl<T: Copy> Table<T> {
    /// A table over `v`'s allocation (its items are dropped).
    pub(super) fn reuse<U>(v: &mut Vec<U>) -> Self {
        Table(recycle(core::mem::take(v), usize::MAX))
    }

    /// Give the allocation back to `v`, unless it grew past
    /// [`SCRATCH_KEEP`].
    pub(super) fn give_back<U>(self, v: &mut Vec<U>) {
        *v = recycle(self.0, SCRATCH_KEEP);
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub(super) fn push(&mut self, v: T) -> Result<(), MatchFailure> {
        self.0.falloc_push(v).or_oom()
    }

    #[inline]
    pub(super) fn pop(&mut self) -> Option<T> {
        self.0.pop()
    }

    #[inline]
    pub(super) fn as_slice(&self) -> &[T] {
        &self.0
    }

    #[inline]
    pub(super) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.0.get_mut(i)
    }

    /// Replace item `i` (< `len`).
    pub(super) fn set(&mut self, i: usize, v: T) -> Result<(), MatchFailure> {
        *self.get_mut(i).ok_or(MatchFailure::Unsupported)? = v;
        Ok(())
    }

    #[inline]
    pub(super) fn get(&self, i: usize) -> Option<T> {
        self.0.get(i).copied()
    }
}

/// The tables and stacks a query leaves behind for the next one - Lexbor's
/// `lxb_selectors_t` keeps its entry and nested-state pools across calls the
/// same way, so a warm call allocates nothing. The Ruby glue keeps one in
/// the process-global selector engine (`selector_cache`), under the GVL, and
/// every entry point ([`select_all`](super::select_all) etc.) takes one.
///
/// Empty between queries: only the capacity carries over, moved between
/// lifetimes by [`recycle`].
pub struct Scratch {
    pub(super) simples: Vec<Step<'static>>,
    pub(super) compounds: Vec<Compound>,
    pub(super) top: Vec<Chain>,
    pub(super) alts: Vec<Chain>,
    pub(super) pending: Vec<(Lists<'static>, u32)>,
    pub(super) names: Vec<Name>,
    pub(super) tasks: Vec<Task<'static>>,
    pub(super) at: Vec<Option<HtmlNode<'static>>>,
    pub(super) cursors: Vec<HasCursor<'static>>,
    pub(super) positions: Positions,
}

/// A [`Scratch`] vector over this many items is dropped rather than kept, so
/// one pathological query does not pin its memory for the process's life.
pub(super) const SCRATCH_KEEP: usize = 1024;

impl Scratch {
    pub const fn new() -> Self {
        Scratch {
            simples: Vec::new(),
            compounds: Vec::new(),
            top: Vec::new(),
            alts: Vec::new(),
            pending: Vec::new(),
            names: Vec::new(),
            tasks: Vec::new(),
            at: Vec::new(),
            cursors: Vec::new(),
            positions: Positions::new(),
        }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch::new()
    }
}

/// `v`, emptied, as a vector of `U` - or an empty one if its capacity is
/// over `keep`. `T` and `U` differ only in a lifetime here, so the collect
/// reuses `v`'s allocation (std's in-place iteration); were it not to, it
/// would make an empty vector - it never allocates.
pub(super) fn recycle<T, U>(mut v: Vec<T>, keep: usize) -> Vec<U> {
    if v.capacity() > keep {
        return Vec::new();
    }
    v.clear();
    v.into_iter().filter_map(|_| None).collect()
}

#[cfg(test)]
mod scratch_tests {
    use super::*;
    use crate::falloc::Reserve;

    /// [`recycle`] carries the allocation across the lifetime change - the
    /// point of [`Scratch`] - and drops one past its keep limit.
    #[test]
    fn recycle_keeps_the_allocation() {
        let byte = 7u8;
        let mut v: Vec<Option<&u8>> = Vec::new();
        v.falloc_reserve(16).expect("reserve");
        v.falloc_push(Some(&byte)).expect("push");
        let (ptr, cap) = (v.as_ptr() as usize, v.capacity());
        let w: Vec<Option<&'static u8>> = recycle(v, usize::MAX);
        assert!(w.is_empty());
        assert_eq!((w.as_ptr() as usize, w.capacity()), (ptr, cap));
        let dropped: Vec<Option<&'static u8>> = recycle(w, 4);
        assert_eq!(dropped.capacity(), 0);
    }
}
