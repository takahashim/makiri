//! Per-document text-extraction index (dom_adapter/text_index.c).
//!
//! Descendant-text aggregation (`Node#text`, the XPath string-value) otherwise
//! walks every node of a subtree chasing pointers through Lexbor's 96-byte
//! nodes; at scale that is cache-bound and dominates the cost. This index
//! removes the per-extraction walk: one build pass records, in document order, a
//! flat array of every TEXT/CDATA node's byte slice, plus a pointer-keyed map
//! from each element or fragment to the contiguous `[start, end)` run of slices
//! its subtree owns. A later `Node#text` is a hash lookup and a single pre-sized
//! memcpy run over a cache-dense slice array, touching no element node at all.
//!
//! # The slices are raw pointers on purpose
//!
//! A borrowed `&[u8]` here would be a stronger claim than the truth. The nodes
//! belong to C, and C's mutation API writes through them outside Rust's borrow
//! checker, so a long-lived Rust reference would assert a lifetime nothing
//! enforces. What actually keeps a cached slice valid is a RUNTIME protocol:
//! every mutation goes through one invalidation hook
//! ([`parsed_text_index_invalidate`]), which drops the whole index, so a
//! slice can never outlive the storage it points into. The arena also never
//! frees a node - detached, never destroyed - so only a mutation can reallocate
//! the text a slice borrows. That protocol is the safety argument; the types
//! record it rather than pretending to prove it.
//!
//! # Fail-closed
//!
//! A build that cannot allocate leaves the index unbuilt and answers 0, and the
//! caller falls back to its own walk. It must never answer a SHORTER run: a
//! truncated text is a well-formed wrong answer, which is exactly what the
//! project's fail-closed rule is about.

#![allow(clippy::missing_safety_doc)]

use core::ffi::{c_int, c_void};

use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::lexbor_abi::{self as lxb, preorder_next, LxbDoc, LxbNode};
use crate::text::BorrowedText;
use crate::xpath::runtime_abi::cache::ptr_hash;

/// `mkr_parsed_t` - the parse handle. Generated, so a field added ahead of `doc`
/// cannot become a silent wrong read here.
type Parsed = lxb::mkr::mkr_parsed_t;

mod ty {
    use crate::lexbor_abi as lxb;
    pub const ELEMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_ELEMENT;
    pub const TEXT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_TEXT;
    pub const CDATA: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_CDATA_SECTION;
    pub const FRAGMENT: u32 = lxb::lxb_dom_node_type_t_LXB_DOM_NODE_TYPE_DOCUMENT_FRAGMENT;
}

extern "C" {
    fn lxb_dom_document_root(doc: *mut LxbDoc) -> *mut LxbNode;
}

/// One container's slice run. `start`/`end` are INDICES into `slices`, not byte
/// offsets, and are `u32` because the build refuses to index a document with
/// more than `u32::MAX` slices (see [`TextIndex::build`]).
#[derive(Clone, Copy)]
struct Range {
    /// The key; null marks an empty slot.
    node: *const LxbNode,
    start: u32,
    end: u32,
}

const EMPTY_RANGE: Range = Range {
    node: core::ptr::null(),
    start: 0,
    end: 0,
};

struct TextIndex {
    /// Document-order TEXT/CDATA slices, borrowed from the arena.
    slices: Vec<BorrowedText>,
    /// `slices.len() + 1` entries; `prefix[i]` is the byte count before slice
    /// `i`. Always non-empty, so an empty `[0, 0)` run over a text-free subtree
    /// reads `prefix[0]` rather than nothing.
    prefix: Vec<usize>,
    /// Container -> slice run, open addressing with linear probing. Empty (cap
    /// 0) when the subtree has no containers.
    ranges: Vec<Range>,
    /// A power of two, or 0.
    ranges_cap: usize,
}

impl TextIndex {
    #[inline]
    fn slot_for(&self, node: *const LxbNode) -> usize {
        (ptr_hash(node) as usize) & (self.ranges_cap - 1)
    }

    /// Insert `node` with `start`; `end` is filled when its subtree closes.
    /// Returns the slot index.
    ///
    /// The table is pre-sized for exactly the container count at load < 3/4, so
    /// it never rehashes mid-build and a free slot always exists - which is what
    /// makes this probe loop terminate.
    fn range_insert(&mut self, node: *const LxbNode, start: u32) -> usize {
        let mut i = self.slot_for(node);
        while !self.ranges[i].node.is_null() {
            i = (i + 1) & (self.ranges_cap - 1);
        }
        self.ranges[i] = Range {
            node,
            start,
            end: start,
        };
        i
    }

    fn range_lookup(&self, node: *const LxbNode) -> Option<Range> {
        if self.ranges_cap == 0 {
            return None;
        }
        let mut i = self.slot_for(node);
        while !self.ranges[i].node.is_null() {
            if self.ranges[i].node == node {
                return Some(self.ranges[i]);
            }
            i = (i + 1) & (self.ranges_cap - 1);
        }
        None
    }
}

#[inline]
unsafe fn is_container(n: *const LxbNode) -> bool {
    (*n).type_ == ty::ELEMENT || (*n).type_ == ty::FRAGMENT
}

/// The non-empty character-data payload of a TEXT/CDATA node, as `(ptr, len)`,
/// or `None`.
///
/// The single "does this node contribute a slice" test, shared by the counting
/// and filling passes so the two-pass sizing and the fill can never disagree
/// about which nodes yield one - a disagreement would size the array for a
/// different set than it fills.
#[inline]
unsafe fn text_slice(n: *const LxbNode) -> Option<(*const u8, usize)> {
    if (*n).type_ != ty::TEXT && (*n).type_ != ty::CDATA {
        return None;
    }
    let d = &(*(n as *const lxb::lxb_dom_character_data_t)).data;
    if d.data.is_null() || d.length == 0 {
        None
    } else {
        Some((d.data, d.length))
    }
}

/// Pass 1: count the text slices and the containers under `root` (inclusive),
/// so each array is sized exactly once.
unsafe fn count(root: *mut LxbNode) -> (usize, usize) {
    let (mut slices, mut containers) = (0usize, 0usize);
    let mut n = root;
    while !n.is_null() {
        if is_container(n) {
            containers += 1;
        } else if text_slice(n).is_some() {
            slices += 1;
        }
        n = preorder_next(n, root);
    }
    (slices, containers)
}

/// An explicit DFS frame. Recursion is avoided so a deep tree cannot exhaust the
/// stack - the same discipline as the attr/element index.
struct Frame {
    /// The next child to visit.
    child: *mut LxbNode,
    /// Index into `ranges` for this open container.
    range: usize,
}

impl TextIndex {
    /// Build over `root` (the document root element). `None` on OOM, which is
    /// fail-closed: the caller walks instead.
    unsafe fn build(root: *mut LxbNode) -> Option<TextIndex> {
        let (nslices, ncont) = count(root);

        /* Run bounds are u32 in the range table. A document with more than
         * u32::MAX text slices is impossible in practice (each is >= 1 byte),
         * but guard it anyway rather than truncate the index. */
        if nslices > u32::MAX as usize {
            return None;
        }

        /* Both arrays are sized EXACTLY here, so every push below lands in
         * reserved capacity and cannot allocate. That is why they use `push`
         * rather than falloc's `mkr_push`: a reserve per text node would make
         * every slice its own injection point in `rake oom` - hundreds of them
         * for one document, all testing the same branch. See clippy.toml on why
         * `push` after a successful reserve is deliberately not banned. */
        let mut t = TextIndex {
            slices: try_vec_with_capacity(nslices)?,
            prefix: try_vec_with_capacity(nslices.checked_add(1)?)?,
            ranges: Vec::new(),
            ranges_cap: 0,
        };
        t.prefix.push(0);

        if ncont > 0 {
            /* Load factor < 3/4: ncont + ncont/2 + 1, rounded up to a power of
             * two. `checked_next_power_of_two` is the fail-closed sizer - a
             * table sized below the element count would never find a free slot
             * under linear probing, and the insert loop would spin forever. */
            let want = ncont.checked_add(ncont >> 1)?.checked_add(1)?;
            let cap = want.checked_next_power_of_two()?;
            t.ranges = try_vec_with_capacity(cap)?;
            t.ranges.resize(cap, EMPTY_RANGE); /* reserved above; cannot allocate */
            t.ranges_cap = cap;
        }

        /* Pass 2: explicit DFS recording each container's slice run. The frame
         * stack is the one array whose size is not known in advance (it is
         * bounded by tree DEPTH, not node count), so it grows through falloc. */
        let mut stack: Vec<Frame> = try_vec_with_capacity(1)?;
        let r = t.range_insert(root, 0);
        stack.push(Frame {
            child: (*root).first_child,
            range: r,
        });

        while let Some(top) = stack.last_mut() {
            let child = top.child;
            if child.is_null() {
                /* Close this subtree. */
                let range = top.range;
                t.ranges[range].end = t.slices.len() as u32;
                stack.pop();
                continue;
            }
            top.child = (*child).next; /* advance the cursor for our return */

            if let Some((ptr, len)) = text_slice(child) {
                /* Compared against the COUNT, not against `capacity()`: a Vec
                 * may reserve more than asked, so capacity would not catch a
                 * count/fill disagreement. One more slice than pass 1 saw means
                 * the tree changed under us - fail closed rather than push past
                 * the reservation. */
                if t.slices.len() == nslices {
                    return None;
                }
                /* The running total is checked rather than wrapped: a wrapped
                 * prefix would make a later `prefix[end] - prefix[start]`
                 * smaller than the bytes actually present, which is a short read
                 * into a pre-sized String. */
                let total = t.prefix[t.slices.len()].checked_add(len)?;
                t.slices.push(unsafe {
                    BorrowedText::from_raw_parts(ptr as *const core::ffi::c_char, len)
                });
                t.prefix.push(total);
            } else if is_container(child) {
                let start = t.slices.len() as u32;
                let r = t.range_insert(child, start);
                /* Reserve only when the stack is actually full. `mkr_push`
                 * consults the injection counter on EVERY call, so pushing
                 * unconditionally made each of a document's containers its own
                 * injection point - 178 for one `rake oom` scenario where the C
                 * had 16, all re-testing one branch. The C grows the same way,
                 * through grow_reserve, and only when it must. */
                if stack.len() == stack.capacity() {
                    let want = crate::falloc::grow_capacity(
                        stack.capacity(),
                        stack.len() + 1,
                        core::mem::size_of::<Frame>(),
                    )?;
                    stack.mkr_reserve_exact(want - stack.len()).ok()?;
                }
                stack.push(Frame {
                    child: (*child).first_child,
                    range: r,
                });
            }
            /* Other kinds (comment / PI / doctype) are childless leaves. */
        }

        Some(t)
    }
}

/* ------------------------------------------------------------------ *
 * public surface                                                     *
 * ------------------------------------------------------------------ */

/// Free an index. NULL-safe, so `parsed_destroy` can call it unconditionally.
pub unsafe fn text_index_free(idx: *mut c_void) {
    if !idx.is_null() {
        drop(Box::from_raw(idx as *mut TextIndex));
    }
}

/// Drop the index so the next query rebuilds it.
///
/// This is the whole safety protocol for the borrowed slices: EVERY mutation
/// reaches here, so no cached slice outlives the storage it points into.
pub unsafe fn parsed_text_index_invalidate(p: *mut Parsed) {
    if p.is_null() || (*p).text_index.is_null() {
        return;
    }
    text_index_free((*p).text_index);
    (*p).text_index = core::ptr::null_mut();
}

/// The document-order run of text slices `node`'s subtree owns.
///
/// Returns 1 with `*out_slices` / `*out_n` / `*out_bytes` set, or 0 - meaning
/// "walk instead", for a node outside the indexed tree (a fragment) or a build
/// that could not allocate. Never a shorter run than the truth.
pub unsafe fn parsed_text_slices(
    p: *mut Parsed,
    node: *const LxbNode,
    out_slices: *mut *const BorrowedText,
    out_n: *mut usize,
    out_bytes: *mut usize,
) -> c_int {
    if p.is_null() || (*p).doc.is_null() || node.is_null() {
        return 0;
    }

    if (*p).text_index.is_null() {
        let root = lxb_dom_document_root((*p).doc as *mut LxbDoc);
        if root.is_null() {
            return 0;
        }
        let built = match TextIndex::build(root) {
            Some(t) => t,
            None => return 0, /* OOM: the caller walks */
        };
        let boxed = match crate::falloc::try_box(built) {
            Ok(b) => b,
            Err(_) => return 0,
        };
        (*p).text_index = Box::into_raw(boxed) as *mut c_void;
    }

    let t = &*((*p).text_index as *const TextIndex);
    let r = match t.range_lookup(node) {
        Some(r) => r,
        None => return 0, /* not in the indexed tree */
    };

    *out_slices = t.slices.as_ptr().add(r.start as usize);
    *out_n = (r.end - r.start) as usize;
    *out_bytes = t.prefix[r.end as usize] - t.prefix[r.start as usize];
    1
}
