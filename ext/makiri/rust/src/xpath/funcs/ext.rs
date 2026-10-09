//! The functions beyond XPath 1.0's own library: Nokogiri's two builtins, in
//! its builtin namespace, and the CSS lowering's internal of-type hooks, whose
//! names no expression can spell. `lookup` in the parent is still the one
//! place that knows which names exist.

#![forbid(unsafe_code)]

use super::*;
use crate::falloc::{try_vec_with_capacity, VecPush};

/* ---------- the Nokogiri builtins ---------- */

/// css-class(haystack, needle): true iff `needle` is a whitespace-separated
/// token of `haystack`. Kept behaviour-identical to libxml2's builtin_css_class.
fn ws_token_match(hay: &[u8], val: &[u8]) -> bool {
    if val.is_empty() {
        return true; /* libxml2 returns non-NULL for an empty val */
    }
    hay.split(|&b| crate::xpath::lex::is_ws(b))
        .any(|t| t == val)
}

pub(super) fn fn_css_class<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    _focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    two::<D, _>(ev, args, |hay, needle| boolean(ws_token_match(hay, needle)))
}

/// local-name-is(name): true iff the context node's qualified name (for HTML the
/// lowercase local name) equals the argument.
pub(super) fn fn_local_name_is<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    args: &[Val<D::Node>],
) -> Answer<D::Node> {
    let doc = ev.doc;
    let want = to_text::<D>(&args[0], ev)?;
    boolean(
        focus
            .node
            .is_some_and(|n| doc.qualified_name(n) == want.as_slice()),
    )
}

/* ---------- the CSS-lowered of-type hooks (XML only) ---------- */

/// One entry of [`SiblingPositions`]: an element child and its 1-based
/// positions, counted from the first sibling and from the last.
#[derive(Clone, Copy)]
struct SiblingPos<N> {
    node: N,
    child: u32,
    child_last: u32,
    of_type: u32,
    of_type_last: u32,
}

/// The sibling positions of one parent's element children, for the CSS
/// lowering's structural pseudo-classes over XML (`:nth-child`,
/// `:nth-of-type`, `:first-of-type`, ...), computed once per parent per
/// evaluation.
///
/// Asked per candidate, each position is a count over the siblings before it:
/// n^2 over a flat list, which a 10,000-entry sitemap or feed is. Charged to
/// the budget that stopped at 50M steps, and uncharged it ran for 15 s at 40k.
/// One pass instead groups the parent's children by expanded name (sorted
/// once) and records every position; lookups follow the candidates, which
/// arrive in document order, through `cursor`.
pub(crate) struct SiblingPositions<N> {
    parent: Option<N>,
    entries: Vec<SiblingPos<N>>,
    cursor: usize,
}

impl<N> SiblingPositions<N> {
    pub(crate) fn new() -> Self {
        SiblingPositions {
            parent: None,
            entries: Vec::new(),
            cursor: 0,
        }
    }
}

/// Which position a hook reads.
#[derive(Clone, Copy)]
enum PosKind {
    Child,
    ChildLast,
    OfType,
    OfTypeLast,
}

/// Recompute `ev.sibling_positions` for `parent`'s element children.
fn fill_positions<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    parent: D::Node,
) -> Result<(), Reported> {
    let doc = ev.doc;
    let oom = |ev: &Evaluation<'e, 'd, D>| {
        err_setf!(
            ev.budget.sink(),
            ErrorKind::Oom,
            "out of memory computing sibling positions"
        )
    };
    let memo = &mut ev.sibling_positions;
    memo.parent = None;
    memo.entries.clear();
    memo.cursor = 0;

    let mut c = doc.first_child(parent);
    let mut child = 0u32;
    while let Some(n) = c {
        ev.budget.charge_op()?;
        if doc.node_type(n) == NodeType::Element {
            child += 1;
            let entry = SiblingPos {
                node: n,
                child,
                child_last: 0,
                of_type: 0,
                of_type_last: 0,
            };
            if ev.sibling_positions.entries.falloc_push(entry).is_err() {
                return Err(oom(ev));
            }
        }
        c = doc.next(n);
    }
    let memo = &mut ev.sibling_positions;
    let len = memo.entries.len() as u32;
    for e in memo.entries.iter_mut() {
        e.child_last = len - e.child + 1;
    }

    /* Group by expanded name - local name, then namespace - keeping document
     * order within a group (the index breaks ties), then number each group. */
    let Some(mut order) = try_vec_with_capacity::<u32>(memo.entries.len()) else {
        return Err(oom(ev));
    };
    order.extend(0..len);
    let memo = &mut ev.sibling_positions;
    /* The key borrows the document, not the memo, so the numbering below may
     * write the entries while keys are held. */
    let key = |entries: &[SiblingPos<D::Node>], i: u32| {
        let n = entries[i as usize].node;
        (doc.local_name(n), doc.ns_uri(n))
    };
    let entries = &memo.entries;
    order.sort_unstable_by(|&a, &b| key(entries, a).cmp(&key(entries, b)).then(a.cmp(&b)));
    let mut i = 0;
    while i < order.len() {
        let k = key(&memo.entries, order[i]);
        let mut j = i + 1;
        while j < order.len() && key(&memo.entries, order[j]) == k {
            j += 1;
        }
        let group = (j - i) as u32;
        for (r, &at) in order[i..j].iter().enumerate() {
            let e = &mut memo.entries[at as usize];
            e.of_type = r as u32 + 1;
            e.of_type_last = group - r as u32;
        }
        i = j;
    }
    memo.parent = Some(parent);
    Ok(())
}

/// The context element's `kind` position among its siblings; 0 for no element,
/// 1 for one without a parent.
fn sibling_pos<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    node: Option<D::Node>,
    kind: PosKind,
) -> Result<f64, Reported> {
    let doc = ev.doc;
    let Some(node) = node else {
        return Ok(0.0);
    };
    if doc.node_type(node) != NodeType::Element {
        return Ok(0.0);
    }
    let Some(parent) = doc.parent(node) else {
        return Ok(1.0);
    };
    if ev.sibling_positions.parent != Some(parent) {
        fill_positions(ev, parent)?;
    }
    /* From the last hit: the same candidate is asked again (`:nth-*` reads its
     * position twice), and the next one is the entry after it. */
    let memo = &mut ev.sibling_positions;
    let len = memo.entries.len();
    for k in 0..len {
        let at = (memo.cursor + k) % len;
        let e = memo.entries[at];
        if e.node == node {
            memo.cursor = at;
            let pos = match kind {
                PosKind::Child => e.child,
                PosKind::ChildLast => e.child_last,
                PosKind::OfType => e.of_type,
                PosKind::OfTypeLast => e.of_type_last,
            };
            return Ok(f64::from(pos));
        }
        ev.budget.charge_op()?;
    }
    Ok(1.0) /* unreachable: `node` is an element child of `parent` */
}

pub(super) fn fn_of_type_pos<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    number(sibling_pos::<D>(ev, focus.node, PosKind::OfType)?)
}

pub(super) fn fn_of_type_pos_last<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    number(sibling_pos::<D>(ev, focus.node, PosKind::OfTypeLast)?)
}

pub(super) fn fn_child_pos<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    number(sibling_pos::<D>(ev, focus.node, PosKind::Child)?)
}

pub(super) fn fn_child_pos_last<'e, 'd, D: Dom<'d>>(
    ev: &mut Evaluation<'e, 'd, D>,
    focus: &Focus<'d, D>,
    _args: &[Val<D::Node>],
) -> Answer<D::Node> {
    number(sibling_pos::<D>(ev, focus.node, PosKind::ChildLast)?)
}
