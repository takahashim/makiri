//! Document order (§5.1) and the per-evaluate index that answers it in O(1).
//!
//! A complete concern with its own data structure and lifecycle: the comparator
//! walks parent chains, and once a single sort is large enough to amortise a
//! full-document walk the index takes over. The C keeps the two apart only by
//! where the lifecycle hooks live (shared) versus the node-dereferencing halves
//! (per-instance); here they are one module because they are one idea.

#![forbid(unsafe_code)]

use super::abi::*;
use super::dom::*;
use super::eval::Evaluation;
use crate::ptr_table::PtrMap;
use crate::token::Token;

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
pub fn doc_order_cmp<'d, D: Dom<'d>>(doc: D, a: D::Node, b: D::Node) -> i32 {
    if a == b {
        return 0;
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
            return 1; /* b is the owner element; its attribute follows it */
        }
        if b_attr && !a_attr {
            return -1;
        }
        if a_attr && b_attr {
            /* Both attributes of one element: the relative order is
             * implementation-defined, so use the attribute list's order. */
            let mut at = doc.first_attr(aa);
            while let Some(x) = at {
                let xn = D::attr_node(x);
                if xn == a {
                    return -1;
                }
                if xn == b {
                    return 1;
                }
                at = doc.attr_next(x);
            }
            return 0;
        }
        return 0;
    }

    let (mut da, mut db) = (depth_of::<D>(doc, aa), depth_of::<D>(doc, bb));
    while da > db {
        let Some(p) = doc.parent(aa) else { return 0 };
        aa = p;
        da -= 1;
    }
    while db > da {
        let Some(p) = doc.parent(bb) else { return 0 };
        bb = p;
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
    /* Climb in lockstep to the children of the common ancestor. */
    loop {
        match (doc.parent(aa), doc.parent(bb)) {
            (Some(pa), Some(pb)) if pa != pb => {
                aa = pa;
                bb = pb;
            }
            (Some(_), Some(_)) => break,
            /* different documents / roots - undefined, keep it stable */
            _ => return 0,
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

/* ---- the index ---- */

/// Pre-order DFS assigning ordinals: the node, then its attributes (before any
/// child), then its descendants - matching `doc_order_cmp`'s placement.
/// Iterative through parent links, so a deep tree cannot overflow the stack,
/// and it stays inside the subtree (it never follows `root`'s next).
fn order_index_walk<'d, D: Dom<'d>>(doc: D, idx: &mut OrderIndex, root: D::Node) -> bool {
    let mut cur = root;
    let mut ord = 0usize;
    loop {
        if !idx.insert(D::token(cur), ord) {
            return false;
        }
        ord += 1;
        /* Only an element has attributes; `first_attr` answers None for the
         * rest, so it is the element test too. */
        let mut a = doc.first_attr(cur);
        while let Some(x) = a {
            if !idx.insert(D::token(D::attr_node(x)), ord) {
                return false;
            }
            ord += 1;
            a = doc.attr_next(x);
        }
        if let Some(c) = doc.first_child(cur) {
            cur = c;
            continue;
        }
        loop {
            if cur == root {
                return true;
            }
            if let Some(s) = doc.next(cur) {
                cur = s;
                break;
            }
            match doc.parent(cur) {
                Some(p) => cur = p,
                None => return true,
            }
        }
    }
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
fn doc_order_cmp_indexed<'d, D: Dom<'d>>(doc: D, idx: &OrderIndex, a: D::Node, b: D::Node) -> i32 {
    if a == b {
        return 0;
    }
    if !idx.built {
        return doc_order_cmp::<D>(doc, a, b);
    }
    match (idx.lookup(D::token(a)), idx.lookup(D::token(b))) {
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
        .all(|w| cmp(&ev.order_index, w[0], w[1]) <= 0)
    {
        return;
    }

    /* Build the index lazily, and only when the sort is large enough to
     * amortise the full-document walk. */
    if !ev.order_index.built && items.len() >= INDEX_BUILD_MIN {
        /* Best-effort: on OOM the parent-chain comparator still serves. */
        order_index_build::<D>(doc, &mut ev.order_index, doc.document_node());
    }

    /* A stable merge sort, so ties - possible only for synthesised nodes that
     * are not in the index - keep insertion order. Rust's sort_by is exactly
     * that, and it falls back to an in-place merge if it cannot allocate,
     * which is the C's qsort fallback without the loss of stability. */
    let idx = &ev.order_index;
    items.sort_by(|x, y| cmp(idx, *x, *y).cmp(&0));
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
