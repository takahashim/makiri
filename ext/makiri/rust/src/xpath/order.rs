//! Document order (§5.1) and the per-evaluate index that answers it in O(1).
//!
//! A complete concern with its own data structure and lifecycle: the comparator
//! walks parent chains, and once a single sort is large enough to amortise a
//! full-document walk the index takes over. The C keeps the two apart only by
//! where the lifecycle hooks live (shared) versus the node-dereferencing halves
//! (per-instance); here they are one module because they are one idea.

use super::abi::*;
use super::dom::*;
use super::eval::Evaluation;
use crate::falloc::try_vec_with_capacity;
use core::ffi::c_void;

#[derive(Clone, Copy)]
struct OrderBucket {
    /// NULL is an empty slot.
    node: *const c_void,
    ord: usize,
}

const EMPTY_BUCKET: OrderBucket = OrderBucket {
    node: core::ptr::null(),
    ord: 0,
};

/// One evaluate's document-order index: node pointer -> its ordinal in a
/// pre-order walk of the document, built at most once, the first time a sort
/// is large enough to pay for the walk.
#[derive(Default)]
pub struct OrderIndex {
    /// Empty, or a power of two at most 3/4 full.
    buckets: Vec<OrderBucket>,
    count: usize,
    built: bool,
}

impl OrderIndex {
    pub const fn new() -> OrderIndex {
        OrderIndex {
            buckets: Vec::new(),
            count: 0,
            built: false,
        }
    }

    /// Insert `(node, ord)`, growing past a 3/4 load factor. False on OOM.
    fn insert(&mut self, node: *const c_void, ord: usize) -> bool {
        if self.buckets.is_empty() || self.count * 4 >= self.buckets.len() * 3 {
            let new_cap = if self.buckets.is_empty() {
                256
            } else {
                match self.buckets.len().checked_mul(2) {
                    Some(c) => c,
                    None => return false,
                }
            };
            let Some(mut new_buckets) = try_vec_with_capacity(new_cap) else {
                return false;
            };
            new_buckets.resize(new_cap, EMPTY_BUCKET);
            let old = core::mem::replace(&mut self.buckets, new_buckets);
            self.count = 0;
            for b in old.iter().filter(|b| !b.node.is_null()) {
                self.put(*b);
            }
        }
        self.put(OrderBucket { node, ord });
        true
    }

    /// Place `b` in its probe run, unless its node is already there. The table
    /// has a free slot.
    fn put(&mut self, b: OrderBucket) {
        let mask = self.buckets.len() - 1;
        let mut j = (ptr_hash(b.node) as usize) & mask;
        loop {
            let slot = &mut self.buckets[j];
            if slot.node.is_null() {
                *slot = b;
                self.count += 1;
                return;
            }
            if slot.node == b.node {
                return; /* already present */
            }
            j = (j + 1) & mask;
        }
    }

    #[inline]
    fn lookup(&self, node: *const c_void) -> Option<usize> {
        if self.buckets.is_empty() {
            return None;
        }
        let mask = self.buckets.len() - 1;
        let mut j = (ptr_hash(node) as usize) & mask;
        loop {
            let slot = &self.buckets[j];
            if slot.node.is_null() {
                return None;
            }
            if slot.node == node {
                return Some(slot.ord);
            }
            j = (j + 1) & mask;
        }
    }
}

/// An attribute sits "with" its owner element for cross-subtree comparisons;
/// only when both anchor to the same element do the attribute-specific rules
/// apply.
unsafe fn anchor_for_cmp<D: Dom>(doc: D::Doc, n: D::Node) -> D::Node {
    if D::node_type(doc, n) == NTYPE_ATTRIBUTE {
        let p = D::parent(doc, n);
        if D::is_null(p) {
            n
        } else {
            p
        }
    } else {
        n
    }
}

unsafe fn depth_of<D: Dom>(doc: D::Doc, mut n: D::Node) -> i32 {
    let mut d = 0;
    while !D::is_null(D::parent(doc, n)) {
        d += 1;
        n = D::parent(doc, n);
    }
    d
}

/// Document order (§5.1): an element, then its attribute nodes, then its
/// children.
///
/// # Safety
/// Both handles must be live nodes of this backend.
pub unsafe fn doc_order_cmp<D: Dom>(doc: D::Doc, a: D::Node, b: D::Node) -> i32 {
    if a == b {
        return 0;
    }
    let mut aa = anchor_for_cmp::<D>(doc, a);
    let mut bb = anchor_for_cmp::<D>(doc, b);

    /* Same anchor: decide by node type. A non-attribute node anchoring to the
     * same element E can only be E itself - any descendant anchors to itself -
     * so the attribute-vs-descendant case is left to the depth walk below. */
    if aa == bb {
        let a_attr = D::node_type(doc, a) == NTYPE_ATTRIBUTE;
        let b_attr = D::node_type(doc, b) == NTYPE_ATTRIBUTE;
        if a_attr && !b_attr {
            return 1; /* b is the owner element; its attribute follows it */
        }
        if b_attr && !a_attr {
            return -1;
        }
        if a_attr && b_attr {
            /* Both attributes of one element: the relative order is
             * implementation-defined, so use the attribute list's order. */
            let mut at = D::first_attr(doc, aa);
            while !D::is_null(at) {
                if at == a {
                    return -1;
                }
                if at == b {
                    return 1;
                }
                at = D::attr_next(doc, at);
            }
            return 0;
        }
        return 0;
    }

    let (mut da, mut db) = (depth_of::<D>(doc, aa), depth_of::<D>(doc, bb));
    while da > db {
        aa = D::parent(doc, aa);
        da -= 1;
    }
    while db > da {
        bb = D::parent(doc, bb);
        db -= 1;
    }
    if aa == bb {
        /* One is an ancestor of the other, and the ancestor comes first. */
        return if aa == anchor_for_cmp::<D>(doc, a) {
            -1
        } else {
            1
        };
    }
    while D::parent(doc, aa) != D::parent(doc, bb) {
        aa = D::parent(doc, aa);
        bb = D::parent(doc, bb);
    }
    if D::is_null(D::parent(doc, aa)) {
        return 0; /* different documents / roots - undefined, keep it stable */
    }
    /* Resolve sibling order by scanning outward from aa and bb in lockstep
     * rather than forward from the parent's first child: the cost is then the
     * distance between them, not the distance from the front. The latter is
     * quadratic when sorting nodes deep in a wide, flat parent - a predicate
     * picking scattered <li> out of a 2000-child <ul>. */
    let (mut fa, mut fb) = (Some(aa), Some(bb));
    loop {
        fa = fa.map(|n| D::next(doc, n)).filter(|n| !D::is_null(*n));
        fb = fb.map(|n| D::next(doc, n)).filter(|n| !D::is_null(*n));
        if fa == Some(bb) {
            return -1; /* bb lies after aa */
        }
        if fb == Some(aa) {
            return 1;
        }
        if fa.is_none() && fb.is_none() {
            return 0; /* unreachable for same-parent nodes */
        }
    }
}

/// The stored pointers, for the passes that reorder a set in place.
///
/// # Safety
/// The set must hold live handles of this backend.
unsafe fn nodeset_items<'a>(ns: *mut NodeSet) -> &'a mut [*mut c_void] {
    if (*ns).count == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut((*ns).items, (*ns).count)
    }
}

/* ---- the index ---- */

/// Pre-order DFS assigning ordinals: the node, then its attributes (before any
/// child), then its descendants - matching `doc_order_cmp`'s placement.
/// Iterative through parent pointers, so a deep tree cannot overflow the stack,
/// and it stays inside the subtree (it never follows `root`'s next).
unsafe fn order_index_walk<D: Dom>(doc: D::Doc, idx: &mut OrderIndex, root: D::Node) -> bool {
    let mut cur = root;
    let mut ord = 0usize;
    while !D::is_null(cur) {
        if !idx.insert(D::to_void(cur), ord) {
            return false;
        }
        ord += 1;
        if D::node_type(doc, cur) == NTYPE_ELEMENT {
            let mut a = D::first_attr(doc, cur);
            while !D::is_null(a) {
                if !idx.insert(D::to_void(a), ord) {
                    return false;
                }
                ord += 1;
                a = D::attr_next(doc, a);
            }
        }
        if !D::is_null(D::first_child(doc, cur)) {
            cur = D::first_child(doc, cur);
            continue;
        }
        while cur != root && D::is_null(D::next(doc, cur)) {
            cur = D::parent(doc, cur);
        }
        if cur == root {
            break;
        }
        cur = D::next(doc, cur);
    }
    true
}

unsafe fn order_index_build<D: Dom>(doc: D::Doc, idx: &mut OrderIndex, root: D::Node) -> bool {
    if idx.built {
        return true;
    }
    if D::is_null(root) {
        return false;
    }
    if !order_index_walk::<D>(doc, idx, root) {
        *idx = OrderIndex::new();
        return false;
    }
    idx.built = true;
    true
}

/// The indexed comparator, falling back to the parent-chain walk on any miss
/// (a synthesised node, or a cross-document compare).
unsafe fn doc_order_cmp_indexed<D: Dom>(
    doc: D::Doc,
    idx: &OrderIndex,
    a: D::Node,
    b: D::Node,
) -> i32 {
    if a == b {
        return 0;
    }
    if !idx.built {
        return doc_order_cmp::<D>(doc, a, b);
    }
    match (idx.lookup(D::to_void(a)), idx.lookup(D::to_void(b))) {
        (Some(oa), Some(ob)) => oa.cmp(&ob) as i32,
        _ => doc_order_cmp::<D>(doc, a, b),
    }
}

/// Below this many nodes, N log N parent-chain compares beat the full-document
/// walk the index needs (D is typically 6000+ nodes on a real page). The
/// crossover measured between 100 and 300; this keeps small unions and
/// reverse-axis dedups off the build path. Once the index is built by a larger
/// sort earlier in the same evaluate, later small sorts reuse it.
const INDEX_BUILD_MIN: usize = 200;

/// Sort a node-set into document order.
///
/// # Safety
/// The set must hold live handles of this backend.
pub unsafe fn nodeset_sort_doc_order<D: Dom>(ev: &mut Evaluation<'_, D>, ns: *mut NodeSet) {
    let doc = ev.doc;
    if ns.is_null() || (*ns).count < 2 {
        return;
    }
    let items = nodeset_items(ns);

    /* Already-sorted fast path. A relative step over a multi-node context
     * (//li/a) collects its forward-axis results context by context, so when the
     * contexts do not nest the concatenation is already in document order and
     * the sort is pure waste. One O(n) scan with the same comparator confirms
     * it, so this can only skip work, never change the result. Reverse axes and
     * interleaved results fail the scan early. */
    let cmp = |idx: &OrderIndex, a: *mut c_void, b: *mut c_void| {
        doc_order_cmp_indexed::<D>(doc, idx, D::from_void(a), D::from_void(b))
    };
    if items
        .windows(2)
        .all(|w| cmp(&ev.order_index, w[0], w[1]) <= 0)
    {
        return;
    }

    /* Build the index lazily, and only when the sort is large enough to
     * amortise the full-document walk. */
    if !ev.order_index.built && items.len() >= INDEX_BUILD_MIN && !ev.cx.document().is_null() {
        /* Best-effort: on OOM the parent-chain comparator still serves. */
        order_index_build::<D>(doc, &mut ev.order_index, D::document_node(doc));
    }

    /* A stable merge sort, so ties - possible only for synthesised nodes that
     * are not in the index - keep insertion order. Rust's sort_by is exactly
     * that, and it falls back to an in-place merge if it cannot allocate,
     * which is the C's qsort fallback without the loss of stability. */
    let idx = &ev.order_index;
    items.sort_by(|x, y| cmp(idx, *x, *y).cmp(&0));
}

/// Sort into document order and drop duplicates.
///
/// # Safety
/// See `nodeset_sort_doc_order`.
pub unsafe fn nodeset_unique_sorted<D: Dom>(ev: &mut Evaluation<'_, D>, ns: *mut NodeSet) {
    if ns.is_null() || (*ns).count < 2 {
        return;
    }
    nodeset_sort_doc_order::<D>(ev, ns);
    let items = nodeset_items(ns);
    let mut w = 1;
    for r in 1..items.len() {
        if items[r] != items[r - 1] {
            items[w] = items[r];
            w += 1;
        }
    }
    (*ns).count = w;
}
