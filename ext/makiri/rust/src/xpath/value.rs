//! The per-backend value model: node string-values
//! (XPath 1.0 §5), the coercions that read a node-set's first node in document
//! order, and the cached string-value lookup.
//!
//! Generic over `Dom`, where the C compiled the same body once per
//! representation. The values themselves (`Val`, `NodeSet`, `Text`) are owned
//! Rust types: they cross into the glue's custom-function bridge and out as the
//! evaluate result, and dropping one frees what it holds.

#![forbid(unsafe_code)]

use super::abi::*;
use super::axis::walk_descendants;
use super::dom::*;
use super::number;
use crate::cbuf::OwnedBuf;
use crate::err_setf;
use crate::falloc::{try_vec_with_capacity, Reserve};
use crate::token::Token;
use core::ops::ControlFlow;

/* ---- the values ---- */

/// An engine-owned string: UTF-8 bytes in a libc allocation, or no allocation
/// at all for the empty string.
///
/// Interior NULs are possible - DOM text may hold U+0000 - so it is only ever
/// read as a slice.
#[derive(Default)]
pub struct Text(Option<OwnedBuf>);

impl Text {
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_ref().map_or(&[], |b| b.as_slice())
    }

    /// The bytes a `Buf` collected.
    pub fn from_buf(buf: OwnedBuf) -> Text {
        Text(Some(buf))
    }

    /// A copy of `bytes`, or None on OOM. The empty string allocates nothing.
    pub fn try_copy(bytes: &[u8]) -> Option<Text> {
        if bytes.is_empty() {
            return Some(Text(None));
        }
        OwnedBuf::copy_from(bytes).map(|b| Text(Some(b)))
    }

    /// Room for `cap` bytes, zeroed, that `fill` writes and reports how many it
    /// used, or None on OOM.
    pub fn try_fill(cap: usize, fill: impl FnOnce(&mut [u8]) -> usize) -> Option<Text> {
        if cap == 0 {
            return Some(Text(None));
        }
        OwnedBuf::fill(cap, fill).map(|b| Text(Some(b)))
    }
}

/// A node-set: nodes in the order the engine collected them.
///
/// Inside the engine the nodes are the backend's own handles (`NodeSet<D::Node>`);
/// the glue sees tokens (`NodeSet<Token>`, the default), and the evaluator
/// converts at that boundary. It grows only through [`push`](Self::push), which
/// holds it to the budget's node-set cap and fails closed on OOM.
pub struct NodeSet<N = Token>(Vec<N>);

impl<N> Default for NodeSet<N> {
    fn default() -> NodeSet<N> {
        NodeSet::new()
    }
}

impl<N> NodeSet<N> {
    pub const fn new() -> NodeSet<N> {
        NodeSet(Vec::new())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The nodes, in order.
    pub fn as_slice(&self) -> &[N] {
        &self.0
    }
    /// The nodes, for the passes that reorder a set in place.
    pub fn as_mut_slice(&mut self) -> &mut [N] {
        &mut self.0
    }
    /// Drop the nodes, keeping the allocation for reuse.
    pub fn clear(&mut self) {
        self.0.clear();
    }
    /// Collapse runs of equal adjacent nodes to one - a de-duplication only
    /// once the set is sorted, which is why its one caller is
    /// `order::nodeset_unique_sorted`, right after the sort.
    pub(in crate::xpath) fn dedup(&mut self)
    where
        N: PartialEq,
    {
        self.0.dedup();
    }

    /// Append `n` within `budget`'s node-set cap.
    pub fn push(&mut self, n: N, budget: &mut Budget) -> Result<(), Reported> {
        budget.check_nodeset_size(self.0.len() + 1)?;
        if self.0.falloc_reserve(1).is_err() {
            return Err(err_setf!(
                budget.sink(),
                Status::Oom,
                "out of memory growing node-set"
            ));
        }
        self.0.push(n);
        Ok(())
    }
}

impl<N: Copy> NodeSet<N> {
    /// A copy, or None on OOM. The nodes belong to the document, so only the
    /// handles are copied.
    pub fn try_clone(&self) -> Option<NodeSet<N>> {
        crate::falloc::try_to_vec(&self.0).map(NodeSet)
    }

    /// Each node passed through `f`, into a new set, or None on OOM.
    pub fn try_map<M>(&self, f: impl FnMut(N) -> M) -> Option<NodeSet<M>> {
        let mut nodes = try_vec_with_capacity(self.0.len())?;
        nodes.extend(self.0.iter().copied().map(f));
        Some(NodeSet(nodes))
    }
}

/// An XPath value (§1): a node-set, a string, a number or a boolean. It owns
/// what it holds. As with [`NodeSet`], the nodes are the backend's handles
/// inside the engine and tokens at the glue.
pub enum Val<N = Token> {
    NodeSet(NodeSet<N>),
    String(Text),
    Number(f64),
    Boolean(bool),
}

/// A value's contents, borrowed.
pub enum ValRef<'a, N = Token> {
    NodeSet(&'a NodeSet<N>),
    String(&'a Text),
    Number(f64),
    Boolean(bool),
}

impl<N> Clone for ValRef<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N> Copy for ValRef<'_, N> {}

impl<N> Default for Val<N> {
    /// The empty node-set.
    fn default() -> Val<N> {
        Val::NodeSet(NodeSet::new())
    }
}

impl<N> Val<N> {
    pub fn nodeset(ns: NodeSet<N>) -> Val<N> {
        Val::NodeSet(ns)
    }
    pub fn string(text: Text) -> Val<N> {
        Val::String(text)
    }
    pub fn number(d: f64) -> Val<N> {
        Val::Number(d)
    }
    pub fn boolean(b: bool) -> Val<N> {
        Val::Boolean(b)
    }

    pub fn get(&self) -> ValRef<'_, N> {
        match self {
            Val::NodeSet(ns) => ValRef::NodeSet(ns),
            Val::String(t) => ValRef::String(t),
            Val::Number(d) => ValRef::Number(*d),
            Val::Boolean(b) => ValRef::Boolean(*b),
        }
    }

    pub fn as_nodeset(&self) -> Option<&NodeSet<N>> {
        match self {
            Val::NodeSet(ns) => Some(ns),
            _ => None,
        }
    }

    pub fn as_nodeset_mut(&mut self) -> Option<&mut NodeSet<N>> {
        match self {
            Val::NodeSet(ns) => Some(ns),
            _ => None,
        }
    }
}

/* ---- the token boundary ---- */

impl<N: Copy> Val<N> {
    /// `self` with each node mapped through `f`, or None on OOM.
    fn try_map_nodes<M>(self, f: impl FnMut(N) -> M) -> Option<Val<M>> {
        Some(match self {
            Val::NodeSet(ns) => Val::NodeSet(ns.try_map(f)?),
            Val::String(t) => Val::String(t),
            Val::Number(d) => Val::Number(d),
            Val::Boolean(b) => Val::Boolean(b),
        })
    }
}

/// `v` as the glue carries it - its nodes as tokens - or None on OOM.
pub fn val_to_tokens<'d, D: Dom<'d>>(v: Val<D::Node>) -> Option<Val> {
    v.try_map_nodes(D::token)
}

/// A copy of `v` as the glue carries it, or None on OOM.
pub fn val_copy_to_tokens<'d, D: Dom<'d>>(v: &Val<D::Node>) -> Option<Val> {
    Some(match v {
        Val::NodeSet(ns) => Val::NodeSet(ns.try_map(D::token)?),
        Val::String(t) => Val::String(Text::try_copy(t.as_slice())?),
        Val::Number(d) => Val::Number(*d),
        Val::Boolean(b) => Val::Boolean(*b),
    })
}

/// `v` with its tokens read back as `doc`'s nodes, or None on OOM.
///
/// Safe because a token is only ever made by `doc`'s own `token` or by the
/// Ruby bridge, which checked the node's document.
pub fn val_from_tokens<'d, D: Dom<'d>>(doc: D, v: Val) -> Option<Val<D::Node>> {
    v.try_map_nodes(|t| doc.resolve_token(t))
}

/// The dynamic context of XPath 1.0 - the "focus": the context node with its
/// 1-based position and the context size. These three always travel together.
///
/// The evaluator is what establishes a focus (a step's per-context predicate
/// pass, and the outermost evaluate); the function library only ever receives
/// one, so it lives here with the other runtime values rather than there.
#[derive(Clone, Copy)]
pub struct Focus<'d, D: Dom<'d>> {
    /// None when the context has no node.
    pub node: Option<D::Node>,
    pub pos: usize,
    pub size: usize,
}

/// Copy `s` into a fresh text, or `Err` with `err` set to `what` on OOM.
pub fn owned_copy(s: &[u8], err: ErrSink, what: &str) -> Result<Text, Reported> {
    Text::try_copy(s).ok_or_else(|| err_setf!(err, Status::Oom, "{}", what))
}

/* ---------- value clone ---------- */

/// Deep-copy `src`. The node-set case copies the handles only: the nodes belong
/// to the document, not to the value.
pub fn val_clone<N: Copy>(src: &Val<N>, err: ErrSink) -> Result<Val<N>, Reported> {
    Ok(match src {
        Val::String(s) => Val::String(owned_copy(
            s.as_slice(),
            err,
            "out of memory cloning string value",
        )?),
        Val::Number(d) => Val::Number(*d),
        Val::Boolean(b) => Val::Boolean(*b),
        Val::NodeSet(ns) => match ns.try_clone() {
            Some(copy) => Val::NodeSet(copy),
            None => {
                return Err(err_setf!(
                    err,
                    Status::Oom,
                    "out of memory cloning node-set"
                ))
            }
        },
    })
}

/* ---------- node string-value (XPath 1.0 §5) ----------
 *
 * Built into a buffer whose ceiling is the per-evaluate byte cap, so an append
 * fails closed past it - there is never a partial or truncated result. */

/// Why a string-value could not be built: the buffer refused (its byte cap, or
/// OOM), or the walk ran out of the evaluation's op budget.
enum Unbuilt {
    Buf(BufError),
    Budget(Reported),
}

/// Append the string-value of every character-data descendant of `node`, in
/// document order.
///
/// Both TEXT and CDATA count as character data (§3 / §5: a CDATA section is
/// text, not a distinct node type). The axis walker is iterative through parent
/// links, so an adversarially deep tree cannot overflow the stack; only an
/// element has children below `node`, so the walk goes into elements only.
///
/// Each visited node is charged to the op budget. The byte cap alone did not
/// bound the work: the walk is over every descendant, text or not, and a
/// predicate runs it once per candidate - `//span[. = 'x']` over 16,000 nested
/// spans walked a quadratic number of empty elements for two seconds.
fn append_text_descendants<'d, D: Dom<'d>>(
    doc: D,
    node: D::Node,
    buf: &mut Buf,
    budget: &Budget,
) -> Result<(), Unbuilt> {
    let flow = walk_descendants::<D, _, _>(doc, node, &mut |n| {
        if let Err(r) = budget.charge_op() {
            return ControlFlow::Break(Unbuilt::Budget(r));
        }
        if !matches!(doc.node_type(n), NTYPE_TEXT | NTYPE_CDATA_SECTION) {
            return ControlFlow::Continue(());
        }
        /* LIMIT or OOM - the caller fails closed */
        match doc.append_own_text(n, buf) {
            Ok(()) => ControlFlow::Continue(()),
            Err(e) => ControlFlow::Break(Unbuilt::Buf(e)),
        }
    });
    match flow {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(e) => Err(e),
    }
}

fn build_string_value<'d, D: Dom<'d>>(
    doc: D,
    node: D::Node,
    buf: &mut Buf,
    budget: &Budget,
) -> Result<(), Unbuilt> {
    if let Some(a) = doc.as_attr(node) {
        let v = doc.attr_value(a);
        return if v.is_empty() {
            Ok(())
        } else {
            buf.append(v).map_err(Unbuilt::Buf)
        };
    }
    match doc.node_type(node) {
        NTYPE_TEXT | NTYPE_CDATA_SECTION | NTYPE_COMMENT | NTYPE_PI => {
            doc.append_own_text(node, buf).map_err(Unbuilt::Buf)
        }
        _ => append_text_descendants::<D>(doc, node, buf, budget),
    }
}

/// Build `node`'s XPath string-value - the one node string-value builder,
/// bounded by `budget`'s `max_string_bytes`. Any failure returns `Err` with the
/// budget's slot set: there is no unbounded or best-effort form to reach for.
pub fn node_to_owned_text<'d, D: Dom<'d>>(
    doc: D,
    node: D::Node,
    budget: &mut Budget,
) -> Result<Text, Reported> {
    let max = budget.limits.max_string_bytes;
    let mut buf = Buf::new(max);
    let built = build_string_value::<D>(doc, node, &mut buf, budget).map_err(|e| match e {
        Unbuilt::Budget(reported) => reported,
        Unbuilt::Buf(BufError::Limit) => err_setf!(
            budget.sink(),
            Status::Limit,
            "string size limit exceeded ({} bytes) while building node string-value",
            max
        ),
        Unbuilt::Buf(_) => err_setf!(
            budget.sink(),
            Status::Oom,
            "out of memory building node string-value"
        ),
    });
    built?;
    let owned = buf.steal().map_err(|_| {
        err_setf!(
            budget.sink(),
            Status::Oom,
            "out of memory building node string-value"
        )
    })?;
    Ok(Text::from_buf(owned))
}

/* ---------- coercions ---------- */

/// string -> number (§4.4): optional leading whitespace, an optional single '-'
/// (no space after it, and no '+'), a Number, optional trailing whitespace.
/// Anything else is NaN. The Number scan is the lexer's, so "0x10" / "1e3" /
/// "INF" all come out NaN - the extent stops early and the leftover trips the
/// end check.
pub fn bytes_to_number(s: &[u8]) -> f64 {
    let s = super::lex::trim_ws(s);
    let (neg, body) = s.strip_prefix(b"-").map_or((false, s), |rest| (true, rest));
    /* The whole of what is left must be one Number: an empty body, a space
     * after the '-', or anything trailing leaves the extent short. */
    if body.is_empty() || number::extent(body) != body.len() {
        return f64::NAN;
    }
    let d = number::from_extent(body);
    if neg {
        -d
    } else {
        d
    }
}

/// value -> number (§4.4) for anything but a node-set, which never allocates
/// and cannot fail. None for a node-set, whose number needs its first node's
/// string-value built under a budget - [`val_to_number_or_fail`].
pub fn scalar_to_number<N>(v: &Val<N>) -> Option<f64> {
    match v.get() {
        ValRef::Number(d) => Some(d),
        ValRef::Boolean(b) => Some(if b { 1.0 } else { 0.0 }),
        ValRef::String(s) => Some(bytes_to_number(s.as_slice())),
        ValRef::NodeSet(_) => None,
    }
}

pub fn val_to_boolean<N>(v: &Val<N>) -> bool {
    match v.get() {
        ValRef::Boolean(b) => b,
        ValRef::Number(d) => !(d == 0.0 || d.is_nan()),
        /* Non-empty, by length (§4.3). A first byte of 0 is U+0000, which DOM
         * text may hold - reading it as a C string's end made "\0abc" false. */
        ValRef::String(s) => !s.as_slice().is_empty(),
        ValRef::NodeSet(ns) => !ns.is_empty(),
    }
}

/// value -> string (§4.2), bounded by `budget`.
pub fn val_to_owned_text_or_fail<'d, D: Dom<'d>>(
    doc: D,
    v: &Val<D::Node>,
    budget: &mut Budget,
) -> Result<Text, Reported> {
    let err = budget.sink();
    match v.get() {
        ValRef::String(s) => {
            let text = s.as_slice();
            budget.check_string_bytes(text.len())?;
            owned_copy(text, err, "out of memory copying string value")
        }
        ValRef::Boolean(b) => {
            let s: &[u8] = if b { b"true" } else { b"false" };
            owned_copy(s, err, "out of memory converting boolean to string")
        }
        ValRef::Number(d) => {
            let what = "out of memory converting number to string";
            if d.is_nan() {
                return owned_copy(b"NaN", err, what);
            }
            if d.is_infinite() {
                let s: &[u8] = if d < 0.0 { b"-Infinity" } else { b"Infinity" };
                return owned_copy(s, err, what);
            }
            if d == 0.0 {
                return owned_copy(b"0", err, what);
            }
            let mut buf = [0u8; 64];
            match number::to_text(d, &mut buf) {
                Some(n) => owned_copy(&buf[..n], err, what),
                None => Err(err_setf!(
                    err,
                    Status::Internal,
                    "number string conversion overflow"
                )),
            }
        }
        ValRef::NodeSet(ns) => {
            /* §4.2: string(node-set) is the string-value of its first node in
             * document order. */
            match ns.as_slice().first() {
                Some(&first) => node_to_owned_text::<D>(doc, first, budget),
                None => owned_copy(b"", err, "out of memory"),
            }
        }
    }
}

/// value -> number, bounded. Only the node-set case can fail (it builds a
/// string-value first).
pub fn val_to_number_or_fail<'d, D: Dom<'d>>(
    doc: D,
    v: &Val<D::Node>,
    budget: &mut Budget,
) -> Result<f64, Reported> {
    if let Some(d) = scalar_to_number(v) {
        return Ok(d);
    }
    /* A node-set: the string-value of its first node in document order, and
     * NaN for an empty one. */
    let first = v.as_nodeset().and_then(|ns| ns.as_slice().first().copied());
    let Some(node) = first else {
        return Ok(f64::NAN);
    };
    let text = node_to_owned_text::<D>(doc, node, budget)?;
    Ok(bytes_to_number(text.as_slice()))
}

/* ---------- the cached string-value of a node ---------- */

/// The cached string-value of `node`, building and caching it on a miss. The
/// text is `.bytes(&ev.str_cache)`.
pub fn cached_node_text<'e, 'd, D: Dom<'d>>(
    ev: &mut super::eval::Evaluation<'e, 'd, D>,
    node: D::Node,
) -> Result<NodeText, Reported> {
    let key = D::token(node);
    if let Some(id) = ev.str_cache.find(key) {
        return Ok(NodeText::Cached(id));
    }
    let text = node_to_owned_text::<D>(ev.doc, node, &mut ev.budget)?;
    ev.str_cache.insert(key, text, &mut ev.budget)
}

/// `number()` of `node`'s cached string-value.
#[inline]
pub fn cached_node_number<'e, 'd, D: Dom<'d>>(
    ev: &mut super::eval::Evaluation<'e, 'd, D>,
    node: D::Node,
) -> Result<f64, Reported> {
    let text = cached_node_text::<D>(ev, node)?;
    Ok(bytes_to_number(text.bytes(&ev.str_cache)))
}

#[cfg(test)]
mod number_tests {
    use super::bytes_to_number;

    #[test]
    fn string_to_number_follows_section_4_4() {
        for (src, want) in [
            (&b"5"[..], 5.0),
            (b" \t5\r\n", 5.0),
            (b"-5", -5.0),
            (b" -1.5 ", -1.5),
            (b"5.", 5.0),
            (b".5", 0.5),
        ] {
            assert_eq!(bytes_to_number(src), want, "{src:?}");
        }
        for src in [
            &b""[..],
            b" ",
            b"-",
            b"- 5",
            b".",
            b"5 5",
            b"5x",
            b"+5",
            b"1e3",
            b"0x10",
            b"\x0c5",
        ] {
            assert!(bytes_to_number(src).is_nan(), "{src:?}");
        }
    }
}
