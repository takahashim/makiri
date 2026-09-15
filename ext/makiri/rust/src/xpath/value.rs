//! The per-backend value model (mkr_xpath_value_body.h): node string-values
//! (XPath 1.0 §5), the coercions that read a node-set's first node, document
//! order, and the cached string-value lookup.
//!
//! Generic over `Dom`, where the C compiled the same body once per
//! representation. The values themselves (`Val`, `NodeSet`, `Text`) are owned
//! Rust types: they cross into the glue's custom-function bridge and out as the
//! evaluate result, and dropping one frees what it holds.

use super::abi::*;
use super::dom::*;
use super::number;
use crate::cbuf::OwnedBuf;
use crate::err_setf;
use crate::falloc::{try_vec_with_capacity, Reserve};
use core::ffi::{c_int, c_void};

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

/// A node as the glue carries it: an erased handle, which only the backend
/// that made it can read back.
pub type Token = *mut c_void;

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
    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    /// Append `n` within `budget`'s node-set cap.
    pub fn push(&mut self, n: N, budget: &mut Budget) -> Result<(), Reported> {
        budget.check_nodeset_size(self.0.len() + 1)?;
        if self.0.mkr_reserve(1).is_err() {
            return Err(err_setf!(
                budget.sink(),
                XP_ERR_OOM,
                "out of memory growing node-set"
            ));
        }
        self.0.push(n);
        Ok(())
    }
}

impl<N: Copy> NodeSet<N> {
    /// Node `i`.
    pub fn get(&self, i: usize) -> N {
        self.0[i]
    }

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

impl NodeSet<Token> {
    /// Append a token within `budget`'s node-set cap. A null token names no
    /// node and is not pushed.
    pub fn push_token(&mut self, token: Token, budget: &mut Budget) -> Result<(), Reported> {
        if token.is_null() {
            return Ok(());
        }
        self.push(token, budget)
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

/// `v` as the glue carries it - its nodes as tokens - or None on OOM.
pub fn val_to_tokens<'d, D: Dom<'d>>(v: Val<D::Node>) -> Option<Val> {
    Some(match v {
        Val::NodeSet(ns) => Val::NodeSet(ns.try_map(D::token)?),
        Val::String(t) => Val::String(t),
        Val::Number(d) => Val::Number(d),
        Val::Boolean(b) => Val::Boolean(b),
    })
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
/// # Safety
/// Every token must name a node of `doc`.
pub unsafe fn val_from_tokens<'d, D: Dom<'d>>(doc: D, v: Val) -> Option<Val<D::Node>> {
    Some(match v {
        Val::NodeSet(ns) => Val::NodeSet(ns.try_map(|t| doc.node(t))?),
        Val::String(t) => Val::String(t),
        Val::Number(d) => Val::Number(d),
        Val::Boolean(b) => Val::Boolean(b),
    })
}

/// The dynamic context of XPath 1.0 - the "focus": the context node with its
/// 1-based position and the context size. These three always travel together.
///
/// The evaluator is what establishes a focus (a step's per-context predicate
/// pass, and the outermost evaluate); the function library only ever receives
/// one, so it lives here with the other runtime values rather than there.
#[derive(Clone, Copy)]
pub struct Focus<'e, D: Dom<'e>> {
    /// None when the context has no node.
    pub node: Option<D::Node>,
    pub pos: usize,
    pub size: usize,
}

/* mkr_status_t */
pub const ST_OK: c_int = 0;
pub const ST_ERR_OOM: c_int = 1;
pub const ST_ERR_LIMIT: c_int = 2;

/// Copy `s` into a fresh text, or `Err` with `err` set to `what` on OOM.
pub fn owned_copy(s: &[u8], err: ErrSink, what: &core::ffi::CStr) -> Result<Text, Reported> {
    Text::try_copy(s).ok_or_else(|| err_setf!(err, XP_ERR_OOM, "{}", what.to_string_lossy()))
}

/* ---------- value clone ---------- */

/// Deep-copy `src`. The node-set case copies the handles only: the nodes belong
/// to the document, not to the value.
pub fn val_clone<N: Copy>(src: &Val<N>, err: ErrSink) -> Result<Val<N>, Reported> {
    Ok(match src {
        Val::String(s) => Val::String(owned_copy(
            s.as_slice(),
            err,
            c"out of memory cloning string value",
        )?),
        Val::Number(d) => Val::Number(*d),
        Val::Boolean(b) => Val::Boolean(*b),
        Val::NodeSet(ns) => match ns.try_clone() {
            Some(copy) => Val::NodeSet(copy),
            None => return Err(err_setf!(err, XP_ERR_OOM, "out of memory cloning node-set")),
        },
    })
}

/* ---------- node string-value (XPath 1.0 §5) ----------
 *
 * Built into a buffer whose ceiling is the per-evaluate byte cap, so an append
 * fails closed past it - there is never a partial or truncated result. */

/// Append the string-value of every character-data descendant of `node`, in
/// document order.
///
/// Both TEXT and CDATA count as character data (§3 / §5: a CDATA section is
/// text, not a distinct node type). The walk is iterative through parent
/// pointers rather than recursive, so an adversarially deep tree cannot
/// overflow the stack; it descends only into elements.
unsafe fn append_text_descendants<'d, D: Dom<'d>>(doc: D, node: D::Node, buf: &mut Buf) -> c_int {
    let mut cur = doc.first_child(node);
    while let Some(n) = cur {
        let t = doc.node_type(n);
        if t == NTYPE_TEXT || t == NTYPE_CDATA_SECTION {
            let st = doc.append_own_text(n, buf);
            if st != ST_OK {
                return st; /* LIMIT or OOM - the caller fails closed */
            }
        }
        if t == NTYPE_ELEMENT {
            if let Some(c) = doc.first_child(n) {
                cur = Some(c);
                continue;
            }
        }
        /* Past `n`'s subtree, without leaving `node`'s. */
        let mut m = n;
        cur = loop {
            if m == node {
                break None;
            }
            if let Some(s) = doc.next(m) {
                break Some(s);
            }
            match doc.parent(m) {
                Some(p) => m = p,
                None => break None,
            }
        };
    }
    ST_OK
}

unsafe fn build_string_value<'d, D: Dom<'d>>(doc: D, node: D::Node, buf: &mut Buf) -> c_int {
    if let Some(a) = doc.as_attr(node) {
        let v = doc.attr_value(a);
        return if v.is_empty() {
            ST_OK
        } else {
            buf_append(buf, v.as_ptr() as *const c_void, v.len())
        };
    }
    match doc.node_type(node) {
        NTYPE_TEXT | NTYPE_CDATA_SECTION | NTYPE_COMMENT | NTYPE_PI => {
            doc.append_own_text(node, buf)
        }
        _ => append_text_descendants::<D>(doc, node, buf),
    }
}

/// Build `node`'s XPath string-value - the one node string-value builder.
///
/// With a budget the build is bounded by its `max_string_bytes` and any failure
/// returns `Err` with its slot set. Without one it is unbounded and
/// best-effort: a failure yields an owned "" and returns `Ok`, because the sole
/// such caller is the NUMBER coercion, and a node whose text overran the ceiling
/// was never a valid number - "" coerces to NaN, which is the right answer
/// anyway.
///
/// # Safety
/// `node` must be live.
pub unsafe fn node_to_owned_text<'d, D: Dom<'d>>(
    doc: D,
    node: D::Node,
    budget: Option<&mut Budget>,
) -> Result<Text, Reported> {
    let max = budget.as_ref().map_or(0, |b| b.limits.max_string_bytes);
    let err = budget.map_or(ErrSink::silent(), |b| b.sink());
    let mut buf = Buf::new(max);
    let st = build_string_value::<D>(doc, node, &mut buf);
    if st == ST_OK {
        if let Ok(owned) = buf.steal() {
            return Ok(Text::from_buf(owned));
        }
        if !err.is_silent() {
            return Err(err_setf!(
                err,
                XP_ERR_OOM,
                "out of memory building node string-value"
            ));
        }
    } else {
        buf.free();
        if !err.is_silent() {
            return Err(if st == ST_ERR_LIMIT {
                err_setf!(
                    err,
                    XP_ERR_LIMIT,
                    "string size limit exceeded ({} bytes) while building node string-value",
                    max
                )
            } else {
                err_setf!(err, XP_ERR_OOM, "out of memory building node string-value")
            });
        }
    }
    /* best-effort: never fail - yield "", which allocates nothing. */
    Ok(Text::default())
}

/* ---------- coercions ---------- */

/// string -> number (§4.4): optional leading whitespace, an optional single '-'
/// (no space after it, and no '+'), a Number, optional trailing whitespace.
/// Anything else is NaN. The Number scan is the lexer's, so "0x10" / "1e3" /
/// "INF" all come out NaN - the extent stops early and the leftover trips the
/// end check.
pub fn bytes_to_number(s: &[u8]) -> f64 {
    let mut i = 0;
    while i < s.len() && super::lex::is_ws(s[i]) {
        i += 1;
    }
    let neg = s.get(i) == Some(&b'-');
    if neg {
        i += 1;
    }
    let extent = number::extent(&s[i..]);
    if extent == 0 {
        return f64::NAN;
    }
    let d = number::from_extent(&s[i..i + extent]);
    i += extent;
    while i < s.len() && super::lex::is_ws(s[i]) {
        i += 1;
    }
    if i != s.len() {
        return f64::NAN; /* trailing garbage */
    }
    if neg {
        -d
    } else {
        d
    }
}

/// The unchecked number coercion: no limits, no errors, NaN for anything that
/// does not coerce.
///
/// # Safety
/// `v` must be a valid value whose node pointers are live.
pub unsafe fn val_to_number_unchecked<'d, D: Dom<'d>>(doc: D, v: &Val<D::Node>) -> f64 {
    match v.get() {
        ValRef::Number(d) => d,
        ValRef::Boolean(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        ValRef::String(s) => bytes_to_number(s.as_slice()),
        ValRef::NodeSet(ns) => {
            if ns.is_empty() {
                return f64::NAN;
            }
            /* string-value of the first node in document order */
            let text = node_text_best_effort::<D>(doc, ns.get(0));
            bytes_to_number(text.as_slice())
        }
    }
}

/// # Safety
/// `v` must be a valid value whose node pointers are live.
pub fn val_to_boolean<N>(v: &Val<N>) -> bool {
    match v.get() {
        ValRef::Boolean(b) => b,
        ValRef::Number(d) => !(d == 0.0 || d.is_nan()),
        /* A string that starts with U+0000 is false, as it was when it was read
         * as a C string. */
        ValRef::String(s) => s.as_slice().first().is_some_and(|&b| b != 0),
        ValRef::NodeSet(ns) => !ns.is_empty(),
    }
}

/// value -> string (§4.2), bounded by `budget`.
///
/// # Safety
/// A node-set value must hold `doc`'s tokens.
pub unsafe fn val_to_owned_text_or_fail<'d, D: Dom<'d>>(
    doc: D,
    v: &Val<D::Node>,
    budget: &mut Budget,
) -> Result<Text, Reported> {
    let err = budget.sink();
    match v.get() {
        ValRef::String(s) => {
            let text = s.as_slice();
            budget.check_string_bytes(text.len())?;
            owned_copy(text, err, c"out of memory copying string value")
        }
        ValRef::Boolean(b) => {
            let s: &[u8] = if b { b"true" } else { b"false" };
            owned_copy(s, err, c"out of memory converting boolean to string")
        }
        ValRef::Number(d) => {
            let what = c"out of memory converting number to string";
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
                    XP_ERR_INTERNAL,
                    "number string conversion overflow"
                )),
            }
        }
        ValRef::NodeSet(ns) => {
            if ns.is_empty() {
                return owned_copy(b"", err, c"out of memory");
            }
            /* §4.2: string(node-set) is the string-value of its first node in
             * document order. */
            node_to_owned_text::<D>(doc, ns.get(0), Some(budget))
        }
    }
}

/// value -> number, bounded. Only the node-set case can fail (it builds a
/// string-value first).
///
/// # Safety
/// `v` must be valid.
pub unsafe fn val_to_number_or_fail<'d, D: Dom<'d>>(
    doc: D,
    v: &Val<D::Node>,
    budget: &mut Budget,
) -> Result<f64, Reported> {
    if let Some(ns) = v.as_nodeset() {
        if ns.is_empty() {
            return Ok(f64::NAN);
        }
        let text = node_to_owned_text::<D>(doc, ns.get(0), Some(budget))?;
        return Ok(bytes_to_number(text.as_slice()));
    }
    Ok(val_to_number_unchecked::<D>(doc, v))
}

/// Build `node`'s string-value with no limit and no error reporting - the
/// best-effort form the NUMBER coercion wants, where an overrun yields "" and
/// "" coerces to NaN, which is the right answer anyway.
#[inline]
unsafe fn node_text_best_effort<'d, D: Dom<'d>>(doc: D, node: D::Node) -> Text {
    node_to_owned_text::<D>(doc, node, None).unwrap_or_default()
}

/* ---------- the cached string-value of a node ---------- */

/// The cached string-value of `node`, building and caching it on a miss. The
/// text is `ev.str_cache.text(id)`.
///
/// # Safety
/// `node` must be a live handle of the evaluation's document.
pub unsafe fn cached_node_text<'d, D: Dom<'d>>(
    ev: &mut super::eval::Evaluation<'d, D>,
    node: D::Node,
) -> Result<TextId, Reported> {
    let key = D::token(node) as *const c_void;
    if let Some(id) = ev.str_cache.find(key) {
        return Ok(id);
    }
    let text = node_to_owned_text::<D>(ev.doc, node, Some(&mut ev.budget))?;
    ev.str_cache.insert(key, text, &mut ev.budget)
}

/// `number()` of `node`'s cached string-value.
///
/// # Safety
/// As [`cached_node_text`].
#[inline]
pub unsafe fn cached_node_number<'d, D: Dom<'d>>(
    ev: &mut super::eval::Evaluation<'d, D>,
    node: D::Node,
) -> Result<f64, Reported> {
    let id = cached_node_text::<D>(ev, node)?;
    Ok(bytes_to_number(ev.str_cache.text(id)))
}
