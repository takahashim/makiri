//! A NodeSet's storage: its node words, a few inline and the rest on a heap
//! the GC is told of, under the set's size cap, and why a push is refused.

#![forbid(unsafe_code)]

use falloc::{Reserve, VecGrow};

use crate::bridge::rb::{Error, Ruby};
use crate::bridge::ruby::makiri_error;
use crate::bridge::wrapper::NodeWord;
use crate::limits::NODE_SET_MAX;

/// How many nodes a set holds in itself before it takes an allocation: a
/// node's children, its ancestors and most query results on a small page fit,
/// so they cost no allocation (nor the protected call one takes) beyond the
/// set's own object.
const INLINE_NODES: usize = 8;

/// The set's node array, under the set's size cap: up to [`INLINE_NODES`] in
/// the set itself, and past that a heap `Vec` that holds them all.
pub(super) struct NodeVec {
    inline: [NodeWord; INLINE_NODES],
    /// The nodes in `inline`; 0 once the set has spilled to `heap`.
    inline_len: usize,
    /// Every node once the set has spilled (its capacity is then non-zero);
    /// empty, holding no allocation, before. On the system allocator through
    /// `falloc`, so `rake oom` fails each growth; its bytes are reported to
    /// Ruby's GC as a Document's arena is (`reported`).
    heap: Vec<NodeWord>,
    /// The heap's bytes as last reported to the GC, given back when the set
    /// is freed.
    pub(super) reported: crate::bridge::typed::External,
}

/// Why a push was refused.
///
/// Deliberately small rather than a `crate::bridge::rb::Error`: a push runs once per node
/// of every result, and returning the large error type from each successful
/// push measured about 2ns a node. Callers convert through `From` on the failure path only.
#[derive(Clone, Copy)]
pub enum PushError {
    SizeLimit,
    /// Ruby's allocator raised `NoMemoryError`, which this answers again.
    NoMemory,
    /// The set is borrowed elsewhere: a broken invariant, since no Ruby can
    /// reach a set while it is being filled.
    Busy,
}

impl From<PushError> for Error {
    fn from(e: PushError) -> Error {
        match e {
            PushError::SizeLimit => makiri_error(format_args!(
                "node set size limit exceeded ({NODE_SET_MAX} nodes)"
            )),
            PushError::NoMemory => Error::new(
                crate::bridge::rb::Builtin::NoMemoryError,
                "failed to allocate memory",
            ),
            PushError::Busy => crate::bridge::ruby::internal_error("a node set borrowed twice"),
        }
    }
}

impl NodeVec {
    pub(super) const fn new() -> Self {
        NodeVec {
            inline: [NodeWord::NULL; INLINE_NODES],
            inline_len: 0,
            heap: Vec::new(),
            reported: crate::bridge::typed::External::new(),
        }
    }

    /// Tell the GC how far the heap's bytes moved since the last report: a
    /// growth is memory Ruby did not allocate and would not otherwise see.
    /// Called after each growth, which is under the GVL (a fill runs in a
    /// method).
    pub(super) fn account(&mut self, ruby: &Ruby) {
        let bytes = self
            .heap
            .capacity()
            .saturating_mul(core::mem::size_of::<NodeWord>());
        self.reported.report(ruby, bytes);
    }

    /// Room for `want` nodes in all on the heap.
    pub(super) fn grow_to(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        want: usize,
    ) -> Result<(), PushError> {
        let more = want.saturating_sub(self.heap.len());
        self.heap
            .falloc_reserve(more)
            .map_err(|()| PushError::NoMemory)?;
        self.account(ruby);
        Ok(())
    }

    pub(super) fn spilled(&self) -> bool {
        self.heap.capacity() != 0
    }

    pub(super) fn len(&self) -> usize {
        if self.spilled() {
            self.heap.len()
        } else {
            self.inline_len
        }
    }

    /// The nodes held outside the set's own memory, for its size report.
    pub(super) fn capacity(&self) -> usize {
        self.heap.capacity()
    }

    pub(super) fn as_slice(&self) -> &[NodeWord] {
        if self.spilled() {
            self.heap.as_slice()
        } else {
            &self.inline[..self.inline_len]
        }
    }

    /// Move to the heap with room for `n` nodes (at least the ones held), in
    /// one allocation.
    pub(super) fn spill(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        n: usize,
    ) -> Result<(), PushError> {
        self.grow_to(ruby, n.max(self.inline_len))?;
        for &w in &self.inline[..self.inline_len] {
            /* Room was reserved above, so this grows nothing. */
            self.heap.falloc_push(w).map_err(|()| PushError::NoMemory)?;
        }
        self.inline_len = 0;
        Ok(())
    }

    /// Append to the heap, growing it (and reporting the growth) when full.
    #[inline]
    pub(super) fn heap_push(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        w: NodeWord,
    ) -> Result<(), PushError> {
        let full = self.heap.len() == self.heap.capacity();
        self.heap.falloc_push(w).map_err(|()| PushError::NoMemory)?;
        if full {
            self.account(ruby);
        }
        Ok(())
    }

    /// Make room for `n` nodes (at most [`NODE_SET_MAX`]) in one allocation,
    /// so the pushes that follow cannot reallocate. Room that fits inline
    /// takes none.
    pub(super) fn reserve(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        n: usize,
    ) -> Result<(), PushError> {
        let n = n.min(NODE_SET_MAX);
        if self.spilled() {
            self.grow_to(ruby, n)
        } else if n > INLINE_NODES {
            self.spill(ruby, n)
        } else {
            Ok(())
        }
    }

    /// Append every node of `nodes`, as [`push`](Self::push) each: room for
    /// the iterator's lower bound is taken first, and a set on the heap is
    /// then filled in one loop, the size cap checked once for the whole run.
    pub(super) fn extend(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        nodes: impl Iterator<Item = NodeWord>,
    ) -> Result<(), PushError> {
        let mut nodes = nodes.peekable();
        self.reserve(ruby, self.len().saturating_add(nodes.size_hint().0))?;
        while nodes.peek().is_some() {
            if self.spilled() {
                let room = NODE_SET_MAX.saturating_sub(self.heap.len());
                for w in nodes.by_ref().take(room) {
                    self.heap_push(ruby, w)?;
                }
                if nodes.peek().is_some() {
                    return Err(PushError::SizeLimit);
                }
                return Ok(());
            }
            if let Some(w) = nodes.next() {
                self.push(ruby, w)?;
            }
        }
        Ok(())
    }

    /// Append one node: `Err` for the size cap or a failed allocation.
    #[inline]
    pub(super) fn push(
        &mut self,
        ruby: &crate::bridge::rb::Ruby,
        node: NodeWord,
    ) -> Result<(), PushError> {
        if self.spilled() {
            if self.heap.len() >= NODE_SET_MAX {
                return Err(PushError::SizeLimit);
            }
            return self.heap_push(ruby, node);
        }
        if let Some(slot) = self.inline.get_mut(self.inline_len) {
            *slot = node;
            self.inline_len += 1;
            return Ok(());
        }
        /* Full inline: spill, with room to grow (NODE_SET_MAX is far above
         * INLINE_NODES, so the cap is not reached here). */
        self.spill(ruby, INLINE_NODES * 4)?;
        self.heap_push(ruby, node)
    }
}
