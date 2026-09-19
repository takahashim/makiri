//! Document order (§5.1) and the per-evaluate index that answers it in O(1).
//!
//! A complete concern with its own data structure and lifecycle: the comparator
//! walks parent chains, and once a single sort is large enough to amortise a
//! full-document walk the index takes over. The C keeps the two apart only by
//! where the lifecycle hooks live (shared) versus the node-dereferencing halves
//! (per-instance); here they are one module because they are one idea.

#![forbid(unsafe_code)]

use super::abi::*;
use super::axis::walk_descendants;
use super::dom::*;
use super::eval::Evaluation;
use crate::falloc::try_vec_with_capacity;
use crate::ptr_table::PtrMap;
use crate::token::Token;
use core::cmp::Ordering;
use core::ops::ControlFlow;

/// One evaluate's document-order index: node token -> its ordinal in a
/// pre-order walk of the document, built at most once, the first time a sort
/// is large enough to pay for the walk.
#[derive(Default)]
pub struct OrderIndex {
    ords: PtrMap<Token, usize>,
    built: bool,
}

impl OrderIndex {
    pub const fn new() -> OrderIndex {
        OrderIndex {
            ords: PtrMap::new(),
            built: false,
        }
    }

    /// Record `(node, ord)`. False on OOM.
    fn insert(&mut self, node: Token, ord: usize) -> bool {
        self.ords.insert(node, ord).is_ok()
    }

    #[inline]
    fn lookup(&self, node: Token) -> Option<usize> {
        self.ords.get(node)
    }
}

/// An attribute sits "with" its owner element for cross-subtree comparisons;
/// only when both anchor to the same element do the attribute-specific rules
/// apply.
fn anchor_for_cmp<'d, D: Dom<'d>>(doc: D, n: D::Node) -> D::Node {
    if doc.node_type(n) == NTYPE_ATTRIBUTE {
        doc.parent(n).unwrap_or(n)
    } else {
        n
    }
}

fn depth_of<'d, D: Dom<'d>>(doc: D, mut n: D::Node) -> i32 {
    let mut d = 0;
    while let Some(p) = doc.parent(n) {
        d += 1;
        n = p;
    }
    d
}

/// Document order (§5.1): an element, then its attribute nodes, then its
/// children.
pub fn doc_order_cmp<'d, D: Dom<'d>>(doc: D, a: D::Node, b: D::Node) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let mut aa = anchor_for_cmp::<D>(doc, a);
    let mut bb = anchor_for_cmp::<D>(doc, b);

    /* Same anchor: decide by node type. A non-attribute node anchoring to the
     * same element E can only be E itself - any descendant anchors to itself -
     * so the attribute-vs-descendant case is left to the depth walk below. */
    if aa == bb {
        let a_attr = doc.node_type(a) == NTYPE_ATTRIBUTE;
        let b_attr = doc.node_type(b) == NTYPE_ATTRIBUTE;
        if a_attr && !b_attr {
            return Ordering::Greater; /* b is the owner element; its attribute follows it */
        }
        if b_attr && !a_attr {
            return Ordering::Less;
        }
        if a_attr && b_attr {
            /* Both attributes of one element: the relative order is
             * implementation-defined, so use the attribute list's order. */
            let mut at = doc.first_attr(aa);
            while let Some(x) = at {
                let xn = D::attr_node(x);
                if xn == a {
                    return Ordering::Less;
                }
                if xn == b {
                    return Ordering::Greater;
                }
                at = doc.attr_next(x);
            }
            return Ordering::Equal;
        }
        return Ordering::Equal;
    }

    let (mut da, mut db) = (depth_of::<D>(doc, aa), depth_of::<D>(doc, bb));
    while da > db {
        let Some(p) = doc.parent(aa) else {
            return Ordering::Equal;
        };
        aa = p;
        da -= 1;
    }
    while db > da {
        let Some(p) = doc.parent(bb) else {
            return Ordering::Equal;
        };
        bb = p;
        db -= 1;
    }
    if aa == bb {
        /* One is an ancestor of the other, and the ancestor comes first. */
        return if aa == anchor_for_cmp::<D>(doc, a) {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    /* Climb in lockstep to the children of the common ancestor. */
    loop {
        match (doc.parent(aa), doc.parent(bb)) {
            (Some(pa), Some(pb)) if pa != pb => {
                aa = pa;
                bb = pb;
            }
            (Some(_), Some(_)) => break,
            /* different documents / roots - undefined, keep it stable */
            _ => return Ordering::Equal,
        }
    }
    /* Resolve sibling order by scanning outward from aa and bb in lockstep
     * rather than forward from the parent's first child: the cost is then the
     * distance between them, not the distance from the front. The latter is
     * quadratic when sorting nodes deep in a wide, flat parent - a predicate
     * picking scattered <li> out of a 2000-child <ul>. */
    let (mut fa, mut fb) = (Some(aa), Some(bb));
    loop {
        fa = fa.and_then(|n| doc.next(n));
        fb = fb.and_then(|n| doc.next(n));
        if fa == Some(bb) {
            return Ordering::Less; /* bb lies after aa */
        }
        if fb == Some(aa) {
            return Ordering::Greater;
        }
        if fa.is_none() && fb.is_none() {
            return Ordering::Equal; /* unreachable for same-parent nodes */
        }
    }
}

/* ---- the index ---- */

/// Assign ordinals in document order: each node, then its attributes (before
/// any child), then its descendants - matching `doc_order_cmp`'s placement.
/// The axis walker is iterative through parent links, so a deep tree cannot
/// overflow the stack, and it stays inside `root`'s subtree. False on OOM.
fn order_index_walk<'d, D: Dom<'d>>(doc: D, idx: &mut OrderIndex, root: D::Node) -> bool {
    let mut ord = 0usize;
    let mut number = |n: D::Node| -> ControlFlow<()> {
        if !idx.insert(D::token(n), ord) {
            return ControlFlow::Break(());
        }
        ord += 1;
        /* Only an element has attributes; `first_attr` answers None for the
         * rest, so it is the element test too. */
        let mut a = doc.first_attr(n);
        while let Some(x) = a {
            if !idx.insert(D::token(D::attr_node(x)), ord) {
                return ControlFlow::Break(());
            }
            ord += 1;
            a = doc.attr_next(x);
        }
        ControlFlow::Continue(())
    };
    number(root).is_continue() && walk_descendants::<D, _, _>(doc, root, &mut number).is_continue()
}

fn order_index_build<'d, D: Dom<'d>>(doc: D, idx: &mut OrderIndex, root: D::Node) -> bool {
    if idx.built {
        return true;
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
fn doc_order_cmp_indexed<'d, D: Dom<'d>>(
    doc: D,
    idx: &OrderIndex,
    a: D::Node,
    b: D::Node,
) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    if !idx.built {
        return doc_order_cmp::<D>(doc, a, b);
    }
    match (idx.lookup(D::token(a)), idx.lookup(D::token(b))) {
        (Some(oa), Some(ob)) => oa.cmp(&ob),
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
pub fn nodeset_sort_doc_order<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    ns: &mut NodeSet<D::Node>,
) {
    let doc = ev.doc;
    if ns.len() < 2 {
        return;
    }
    let items = ns.as_mut_slice();

    /* Already-sorted fast path. A relative step over a multi-node context
     * (//li/a) collects its forward-axis results context by context, so when the
     * contexts do not nest the concatenation is already in document order and
     * the sort is pure waste. One O(n) scan with the same comparator confirms
     * it, so this can only skip work, never change the result. Reverse axes and
     * interleaved results fail the scan early. */
    let cmp = |idx: &OrderIndex, a: D::Node, b: D::Node| doc_order_cmp_indexed::<D>(doc, idx, a, b);
    if items
        .windows(2)
        .all(|w| cmp(&ev.order_index, w[0], w[1]) != Ordering::Greater)
    {
        return;
    }

    /* Build the index lazily, and only when the sort is large enough to
     * amortise the full-document walk. */
    if !ev.order_index.built && items.len() >= INDEX_BUILD_MIN {
        /* Best-effort: on OOM the parent-chain comparator still serves. */
        order_index_build::<D>(doc, &mut ev.order_index, doc.document_node());
    }

    let idx = &ev.order_index;
    merge_sort(items, |x, y| cmp(idx, *x, *y));
}

/// A stable natural merge sort, its scratch space from falloc.
///
/// Not std's `sort_by`, which takes its scratch buffer from the global
/// allocator and ABORTS the process when it cannot - outside falloc, so `rake
/// oom` would never see it. Not `sort_unstable_by` alone either, which cannot
/// use what these inputs usually are: a union, or a step over several
/// contexts, is a few runs already in document order, and merging runs costs
/// O(n) per pass where an unstable sort pays O(n log n) comparisons regardless.
///
/// When the scratch space cannot be had, the sort falls back to the unstable,
/// in-place one: still correct, since two nodes compare Equal only when they
/// are not comparable at all, so there is no order among ties to keep.
fn merge_sort<T: Copy>(items: &mut [T], mut cmp: impl FnMut(&T, &T) -> Ordering) {
    let n = items.len();
    let Some(mut scratch) = try_vec_with_capacity::<T>(n) else {
        items.sort_unstable_by(cmp);
        return;
    };
    scratch.extend_from_slice(items); /* reserved above; cannot allocate */

    /* The end of the non-descending run that starts at `i`. */
    let run_end = |a: &[T], i: usize, cmp: &mut dyn FnMut(&T, &T) -> Ordering| {
        let mut j = i + 1;
        while j < a.len() && cmp(&a[j - 1], &a[j]) != Ordering::Greater {
            j += 1;
        }
        j
    };

    /* Each pass merges neighbouring runs of `items` into `scratch` and copies
     * back, halving the run count, until one run is left. */
    loop {
        let mut runs = 0usize;
        let mut i = 0usize;
        while i < n {
            let mid = run_end(items, i, &mut cmp);
            let end = if mid < n {
                run_end(items, mid, &mut cmp)
            } else {
                n
            };
            /* Stable: on a tie the left run's element goes first. */
            let (mut l, mut r, mut o) = (i, mid, i);
            while l < mid && r < end {
                if cmp(&items[r], &items[l]) == Ordering::Less {
                    scratch[o] = items[r];
                    r += 1;
                } else {
                    scratch[o] = items[l];
                    l += 1;
                }
                o += 1;
            }
            scratch[o..o + (mid - l)].copy_from_slice(&items[l..mid]);
            o += mid - l;
            scratch[o..o + (end - r)].copy_from_slice(&items[r..end]);
            runs += 1;
            i = end;
        }
        items.copy_from_slice(&scratch);
        if runs <= 1 {
            return;
        }
    }
}

/// Sort into document order and drop duplicates.
pub fn nodeset_unique_sorted<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    ns: &mut NodeSet<D::Node>,
) {
    if ns.len() < 2 {
        return;
    }
    nodeset_sort_doc_order::<D>(ev, ns);
    let items = ns.as_mut_slice();
    let mut w = 1;
    for r in 1..items.len() {
        if items[r] != items[r - 1] {
            items[w] = items[r];
            w += 1;
        }
    }
    ns.truncate(w);
}

#[cfg(test)]
mod tests {
    use super::merge_sort;
    use core::cmp::Ordering;

    /// Pairs of (key, original position), in a deterministic scramble.
    fn scrambled(n: usize, keys: u32) -> Vec<(u32, usize)> {
        let mut x = 0x2545_f491_u32;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x % keys, i)
            })
            .collect()
    }

    #[test]
    fn merge_sort_is_a_stable_sort() {
        for (n, keys) in [(0, 1), (1, 1), (2, 2), (17, 3), (1000, 7), (4096, 4096)] {
            let mut got = scrambled(n, keys);
            let mut want = got.clone();
            #[allow(clippy::disallowed_methods)] /* the reference answer */
            want.sort_by_key(|&(k, _)| k);
            merge_sort(&mut got, |a, b| a.0.cmp(&b.0));
            assert_eq!(got, want, "n={n} keys={keys}");
        }
    }

    #[test]
    fn merge_sort_merges_presorted_runs() {
        /* The shape a union hands it: two runs, each already in order. */
        let mut v: Vec<u32> = (0..500).step_by(2).chain((1..500).step_by(2)).collect();
        let mut compares = 0usize;
        merge_sort(&mut v, |a, b| {
            compares += 1;
            a.cmp(b)
        });
        assert!(v.windows(2).all(|w| w[0].cmp(&w[1]) != Ordering::Greater));
        /* One pass to find the two runs, one to merge them, one to confirm. */
        assert!(compares < 3 * v.len(), "{compares} compares");
    }
}
