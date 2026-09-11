//! The per-backend value model (mkr_xpath_value_body.h): node string-values
//! (XPath 1.0 §5), the coercions that read a node-set's first node, document
//! order, and the string-value cache's node-keyed insert.
//!
//! Generic over `Dom`, which is what the C achieves by compiling the same body
//! once per representation. The values themselves (`mkr_val_t`, `mkr_nodeset_t`)
//! stay the C types: they cross into the glue's custom-function bridge and out
//! as the evaluate result, so their layout is ABI.

use super::abi::*;
use super::dom::*;
use super::number;
use super::own::Text;
use crate::err_setf;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

/* mkr_xpath_type_t */
pub const T_NODESET: u32 = 0;
pub const T_STRING: u32 = 1;
pub const T_NUMBER: u32 = 2;
pub const T_BOOLEAN: u32 = 3;

/* mkr_status_t */
pub const ST_OK: c_int = 0;
pub const ST_ERR_OOM: c_int = 1;
pub const ST_ERR_LIMIT: c_int = 2;

/// An empty `mkr_val_t` of the given type; the union starts zeroed, which is a
/// valid empty node-set, a 0.0, a false, and a NULL string.
pub fn val_zero(type_: u32) -> Val {
    Val { type_, u: ValU { nodeset: NodeSet { items: ptr::null_mut(), count: 0, capacity: 0 } } }
}

pub fn val_number(d: f64) -> Val {
    let mut v = val_zero(T_NUMBER);
    v.u.number = d;
    v
}

pub fn val_boolean(b: bool) -> Val {
    let mut v = val_zero(T_BOOLEAN);
    v.u.boolean = c_int::from(b);
    v
}

/// A borrowed view of a `mkr_owned_text_t`, empty when the pointer is NULL.
///
/// # Safety
/// `t` must name live bytes for `'a`.
pub unsafe fn owned_bytes<'a>(t: OwnedText) -> &'a [u8] {
    if t.ptr.is_null() || t.len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(t.ptr as *const u8, t.len)
    }
}

/// Copy `s` into a fresh owned text. Returns false with `*err` set on OOM.
///
/// # Safety
/// `out` must be a writable `mkr_owned_text_t`.
pub unsafe fn owned_copy(out: *mut OwnedText, s: &[u8], err: *mut Error, what: &[u8]) -> bool {
    let t = VerifiedText { ptr: s.as_ptr() as *const c_char, len: s.len() };
    mkr_owned_text_from_borrowed_copy(out, t, err, what.as_ptr() as *const c_char) == 0
}

/* ---------- value clone ---------- */

/// Deep-copy `src` into `dst`. The node-set case copies the pointer array only:
/// the nodes belong to the document, not to the value.
///
/// # Safety
/// Both must point at valid `mkr_val_t`; `dst` is overwritten without being
/// cleared first, so the caller owns whatever was in it.
pub unsafe fn val_clone(src: *const Val, dst: *mut Val, err: *mut Error) -> bool {
    *dst = val_zero((*src).type_);
    match (*src).type_ {
        T_STRING => {
            let mut text = OwnedText { ptr: ptr::null_mut(), len: 0 };
            if !owned_copy(&mut text, owned_bytes((*src).u.string), err, b"out of memory cloning string value\0") {
                return false;
            }
            mkr_val_set_owned_text(dst, text);
            true
        }
        T_NUMBER => {
            (*dst).u.number = (*src).u.number;
            true
        }
        T_BOOLEAN => {
            (*dst).u.boolean = (*src).u.boolean;
            true
        }
        T_NODESET => {
            let n = (*src).u.nodeset.count;
            mkr_nodeset_init(&raw mut (*dst).u.nodeset);
            if n == 0 {
                return true;
            }
            let items = mkr_reallocarray(ptr::null_mut(), n, core::mem::size_of::<*mut c_void>())
                as *mut *mut c_void;
            if items.is_null() {
                err_setf!(err, XP_ERR_OOM, "out of memory cloning node-set");
                return false;
            }
            ptr::copy_nonoverlapping((*src).u.nodeset.items, items, n);
            (*dst).u.nodeset.items = items;
            (*dst).u.nodeset.count = n;
            (*dst).u.nodeset.capacity = n;
            true
        }
        _ => {
            err_setf!(err, XP_ERR_INTERNAL, "mkr_val_clone: unknown value type");
            false
        }
    }
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
unsafe fn append_text_descendants<D: Dom>(node: D::Node, buf: *mut Buf) -> c_int {
    let mut cur = D::first_child(node);
    while !D::is_null(cur) {
        let t = D::node_type(cur);
        if t == NTYPE_TEXT || t == NTYPE_CDATA_SECTION {
            let st = append_own_text::<D>(cur, buf);
            if st != ST_OK {
                return st; /* LIMIT or OOM - the caller fails closed */
            }
        }
        if t == NTYPE_ELEMENT && !D::is_null(D::first_child(cur)) {
            cur = D::first_child(cur);
            continue;
        }
        while cur != node && D::is_null(D::next(cur)) {
            cur = D::parent(cur);
        }
        if cur == node {
            return ST_OK;
        }
        cur = D::next(cur);
    }
    ST_OK
}

unsafe fn append_own_text<D: Dom>(node: D::Node, buf: *mut Buf) -> c_int {
    let s = D::own_text(node);
    if s.is_empty() {
        return ST_OK;
    }
    mkr_buf_append(buf, s.as_ptr() as *const c_void, s.len())
}

unsafe fn build_string_value<D: Dom>(node: D::Node, buf: *mut Buf) -> c_int {
    if D::is_null(node) {
        return ST_OK;
    }
    match D::node_type(node) {
        NTYPE_ATTRIBUTE => {
            let v = D::attr_value(node);
            if v.is_empty() {
                ST_OK
            } else {
                mkr_buf_append(buf, v.as_ptr() as *const c_void, v.len())
            }
        }
        NTYPE_TEXT | NTYPE_CDATA_SECTION | NTYPE_COMMENT | NTYPE_PI => {
            append_own_text::<D>(node, buf)
        }
        _ => append_text_descendants::<D>(node, buf),
    }
}

/// Build `node`'s XPath string-value into `out` - the one node string-value
/// builder.
///
/// With `err` non-null the build is bounded by `limits.max_string_bytes` and any
/// failure returns false with `*err` set. With `err` null it is best-effort: a
/// failure yields an owned "" and returns true, because the sole such caller is
/// the NUMBER coercion, and a node whose text overran the ceiling was never a
/// valid number - "" coerces to NaN, which is the right answer anyway.
///
/// # Safety
/// `node` must be live; `out` writable.
pub unsafe fn node_to_owned_text<D: Dom>(
    node: D::Node,
    limits: *mut Limits,
    err: *mut Error,
    out: *mut OwnedText,
) -> bool {
    *out = OwnedText { ptr: ptr::null_mut(), len: 0 };
    let mut buf = Buf::new(if limits.is_null() { 0 } else { (*limits).max_string_bytes });
    let st = build_string_value::<D>(node, &mut buf);
    if st == ST_OK {
        let mut len = 0usize;
        let p = mkr_buf_steal(&mut buf, &mut len);
        if !p.is_null() {
            (*out).ptr = p;
            (*out).len = len;
            return true;
        }
        if !err.is_null() {
            err_setf!(err, XP_ERR_OOM, "out of memory building node string-value");
            return false;
        }
    } else {
        buf.free();
        if !err.is_null() {
            if st == ST_ERR_LIMIT {
                err_setf!(
                    err,
                    XP_ERR_LIMIT,
                    "string size limit exceeded ({} bytes) while building node string-value",
                    (*limits).max_string_bytes
                );
            } else {
                err_setf!(err, XP_ERR_OOM, "out of memory building node string-value");
            }
            return false;
        }
    }
    /* best-effort: never fail - yield an owned "". */
    owned_copy(out, b"", ptr::null_mut(), b"\0");
    true
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
pub unsafe fn val_to_number_unchecked<D: Dom>(v: *const Val) -> f64 {
    match (*v).type_ {
        T_NUMBER => (*v).u.number,
        T_BOOLEAN => {
            if (*v).u.boolean != 0 {
                1.0
            } else {
                0.0
            }
        }
        T_STRING => bytes_to_number(owned_bytes((*v).u.string)),
        T_NODESET => {
            if (*v).u.nodeset.count == 0 {
                return f64::NAN;
            }
            /* string-value of the first node in document order */
            let text = node_text_best_effort::<D>(nodeset_at::<D>(&(*v).u.nodeset, 0));
            bytes_to_number(text.as_slice())
        }
        _ => f64::NAN,
    }
}

/// # Safety
/// `v` must be a valid value whose node pointers are live.
pub unsafe fn val_to_boolean(v: *const Val) -> bool {
    match (*v).type_ {
        T_BOOLEAN => (*v).u.boolean != 0,
        T_NUMBER => !((*v).u.number == 0.0 || (*v).u.number.is_nan()),
        T_STRING => !(*v).u.string.ptr.is_null() && *(*v).u.string.ptr != 0,
        T_NODESET => (*v).u.nodeset.count > 0,
        _ => false,
    }
}

/// A bounded `core::fmt::Write` sink. An overflow is an error rather than a
/// truncation: a cut-short number string is a wrong answer, and the C checked
/// its `snprintf` return for exactly that reason.
struct Fixed<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for Fixed<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if s.len() > self.buf.len() - self.len {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}

impl<'a> Fixed<'a> {
    fn new(buf: &'a mut [u8]) -> Fixed<'a> {
        Fixed { buf, len: 0 }
    }
    fn written(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Trailing zeros (and a bare trailing '.') dropped from a decimal run, which is
/// what `%g` does. A run with no '.' is returned unchanged.
fn strip_zeros(s: &[u8]) -> &[u8] {
    if !s.contains(&b'.') {
        return s;
    }
    let s = &s[..s.len() - s.iter().rev().take_while(|&&b| b == b'0').count()];
    s.strip_suffix(b".").unwrap_or(s)
}

/// Format a number the way XPath's `string()` does (§4.2): an integral value in
/// range prints as an integer, everything else as C's `%.15g`.
///
/// Returns the byte length, or None if `out` was too small - which the caller
/// turns into an INTERNAL error rather than emitting a truncated number.
/// Allocation-free: this is on the value path of an engine that reports OOM as a
/// status, so it must not be able to abort on a failed allocation instead.
fn number_to_text(d: f64, out: &mut [u8]) -> Option<usize> {
    use core::fmt::Write;
    const P: i32 = 15;

    if d == d.trunc() && d.abs() < 1e15 {
        let mut w = Fixed::new(out);
        write!(w, "{}", d as i64).ok()?;
        return Some(w.len);
    }

    /* %.15g picks exponential when the decimal exponent is below -4 or at least
     * the precision, and strips trailing zeros either way. */
    let exp = if d == 0.0 { 0 } else { d.abs().log10().floor() as i32 };
    let mut scratch = [0u8; 64];

    if (-4..P).contains(&exp) {
        let mut w = Fixed::new(&mut scratch);
        write!(w, "{:.*}", (P - 1 - exp).max(0) as usize, d).ok()?;
        let text = strip_zeros(w.written());
        if text.len() > out.len() {
            return None;
        }
        out[..text.len()].copy_from_slice(text);
        return Some(text.len());
    }

    let mut w = Fixed::new(&mut scratch);
    write!(w, "{:.*e}", (P - 1) as usize, d).ok()?;
    /* Rust writes "1.5e20"; C writes "1.5e+20". */
    let written = w.written();
    let at = written.iter().position(|&b| b == b'e')?;
    let mantissa = strip_zeros(&written[..at]);
    let ev: i32 = core::str::from_utf8(&written[at + 1..]).ok()?.parse().ok()?;

    let mut o = Fixed::new(out);
    o.write_str(core::str::from_utf8(mantissa).ok()?).ok()?;
    write!(o, "e{}{:02}", if ev < 0 { '-' } else { '+' }, ev.abs()).ok()?;
    Some(o.len)
}

/// value -> string (§4.2), bounded by `limits` when it is non-null.
///
/// # Safety
/// `v` may be null (yields ""); `out` must be writable.
pub unsafe fn val_to_owned_text_or_fail<D: Dom>(
    v: *const Val,
    limits: *mut Limits,
    err: *mut Error,
    out: *mut OwnedText,
) -> bool {
    *out = OwnedText { ptr: ptr::null_mut(), len: 0 };
    if v.is_null() {
        return owned_copy(out, b"", err, b"out of memory converting value to string\0");
    }
    match (*v).type_ {
        T_STRING => {
            let text = owned_bytes((*v).u.string);
            if !limits.is_null() && mkr_limit_check_string_bytes(limits, text.len(), err) != 0 {
                return false;
            }
            owned_copy(out, text, err, b"out of memory copying string value\0")
        }
        T_BOOLEAN => {
            let s: &[u8] = if (*v).u.boolean != 0 { b"true" } else { b"false" };
            owned_copy(out, s, err, b"out of memory converting boolean to string\0")
        }
        T_NUMBER => {
            let d = (*v).u.number;
            let what = b"out of memory converting number to string\0";
            if d.is_nan() {
                return owned_copy(out, b"NaN", err, what);
            }
            if d.is_infinite() {
                let s: &[u8] = if d < 0.0 { b"-Infinity" } else { b"Infinity" };
                return owned_copy(out, s, err, what);
            }
            if d == 0.0 {
                return owned_copy(out, b"0", err, what);
            }
            let mut buf = [0u8; 64];
            match number_to_text(d, &mut buf) {
                Some(n) => owned_copy(out, &buf[..n], err, what),
                None => {
                    err_setf!(err, XP_ERR_INTERNAL, "number string conversion overflow");
                    false
                }
            }
        }
        T_NODESET => {
            if (*v).u.nodeset.count == 0 {
                return owned_copy(out, b"", err, b"out of memory\0");
            }
            /* §4.2: string(node-set) is the string-value of its first node in
             * document order. */
            node_to_owned_text::<D>(nodeset_at::<D>(&(*v).u.nodeset, 0), limits, err, out)
        }
        _ => {
            err_setf!(err, XP_ERR_INTERNAL, "unknown value type");
            false
        }
    }
}

/// value -> number, bounded. Only the node-set case can fail (it builds a
/// string-value first).
///
/// # Safety
/// `v` and `out` must be valid.
pub unsafe fn val_to_number_or_fail<D: Dom>(
    v: *const Val,
    limits: *mut Limits,
    err: *mut Error,
    out: *mut f64,
) -> bool {
    if (*v).type_ == T_NODESET {
        if (*v).u.nodeset.count == 0 {
            *out = f64::NAN;
            return true;
        }
        let mut text = Text::new();
        if !node_to_owned_text::<D>(nodeset_at::<D>(&(*v).u.nodeset, 0), limits, err, text.as_mut())
        {
            return false;
        }
        *out = bytes_to_number(text.as_slice());
        return true;
    }
    *out = val_to_number_unchecked::<D>(v);
    true
}

/* ---------- node-set element access ---------- */

/// The node-set stores `void *`; this is where one becomes a backend handle.
///
/// # Safety
/// `i` must be within `ns.count`, and the set must hold handles of this
/// backend's representation - which the engine kind selects.
#[inline]
pub unsafe fn nodeset_at<D: Dom>(ns: *const NodeSet, i: usize) -> D::Node {
    debug_assert!(i < (*ns).count);
    D::from_void(*(*ns).items.add(i))
}

/// The stored pointers, for the passes that reorder the set in place.
///
/// # Safety
/// See `nodeset_at`.
#[inline]
unsafe fn nodeset_items<'a>(ns: *mut NodeSet) -> &'a mut [*mut c_void] {
    if (*ns).count == 0 {
        &mut []
    } else {
        core::slice::from_raw_parts_mut((*ns).items, (*ns).count)
    }
}

/// Build `node`'s string-value with no limit and no error reporting - the
/// best-effort form the NUMBER coercion wants, where an overrun yields "" and
/// "" coerces to NaN, which is the right answer anyway.
#[inline]
unsafe fn node_text_best_effort<D: Dom>(node: D::Node) -> Text {
    let mut t = Text::new();
    node_to_owned_text::<D>(node, ptr::null_mut(), ptr::null_mut(), t.as_mut());
    t
}

/* ---------- document order ---------- */

/// An attribute sits "with" its owner element for cross-subtree comparisons;
/// only when both anchor to the same element do the attribute-specific rules
/// apply.
unsafe fn anchor_for_cmp<D: Dom>(n: D::Node) -> D::Node {
    if D::node_type(n) == NTYPE_ATTRIBUTE {
        let p = D::parent(n);
        if D::is_null(p) {
            n
        } else {
            p
        }
    } else {
        n
    }
}

unsafe fn depth_of<D: Dom>(mut n: D::Node) -> i32 {
    let mut d = 0;
    while !D::is_null(D::parent(n)) {
        d += 1;
        n = D::parent(n);
    }
    d
}

/// Document order (§5.1): an element, then its attribute nodes, then its
/// children.
///
/// # Safety
/// Both handles must be live nodes of this backend.
pub unsafe fn doc_order_cmp<D: Dom>(a: D::Node, b: D::Node) -> i32 {
    if a == b {
        return 0;
    }
    let mut aa = anchor_for_cmp::<D>(a);
    let mut bb = anchor_for_cmp::<D>(b);

    /* Same anchor: decide by node type. A non-attribute node anchoring to the
     * same element E can only be E itself - any descendant anchors to itself -
     * so the attribute-vs-descendant case is left to the depth walk below. */
    if aa == bb {
        let a_attr = D::node_type(a) == NTYPE_ATTRIBUTE;
        let b_attr = D::node_type(b) == NTYPE_ATTRIBUTE;
        if a_attr && !b_attr {
            return 1; /* b is the owner element; its attribute follows it */
        }
        if b_attr && !a_attr {
            return -1;
        }
        if a_attr && b_attr {
            /* Both attributes of one element: the relative order is
             * implementation-defined, so use the attribute list's order. */
            let mut at = D::first_attr(aa);
            while !D::is_null(at) {
                if at == a {
                    return -1;
                }
                if at == b {
                    return 1;
                }
                at = D::attr_next(at);
            }
            return 0;
        }
        return 0;
    }

    let (mut da, mut db) = (depth_of::<D>(aa), depth_of::<D>(bb));
    while da > db {
        aa = D::parent(aa);
        da -= 1;
    }
    while db > da {
        bb = D::parent(bb);
        db -= 1;
    }
    if aa == bb {
        /* One is an ancestor of the other, and the ancestor comes first. */
        return if aa == anchor_for_cmp::<D>(a) { -1 } else { 1 };
    }
    while D::parent(aa) != D::parent(bb) {
        aa = D::parent(aa);
        bb = D::parent(bb);
    }
    if D::is_null(D::parent(aa)) {
        return 0; /* different documents / roots - undefined, keep it stable */
    }
    /* Resolve sibling order by scanning outward from aa and bb in lockstep
     * rather than forward from the parent's first child: the cost is then the
     * distance between them, not the distance from the front. The latter is
     * quadratic when sorting nodes deep in a wide, flat parent - a predicate
     * picking scattered <li> out of a 2000-child <ul>. */
    let (mut fa, mut fb) = (Some(aa), Some(bb));
    loop {
        fa = fa.map(|n| D::next(n)).filter(|n| !D::is_null(*n));
        fb = fb.map(|n| D::next(n)).filter(|n| !D::is_null(*n));
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

/* ---- the per-evaluate document-order index ---- */

/// Insert `(node, ord)`, growing past a 3/4 load factor. False on OOM.
unsafe fn order_index_insert<D: Dom>(idx: *mut OrderIndex, node: D::Node, ord: usize) -> bool {
    if (*idx).cap == 0 || (*idx).count * 4 >= (*idx).cap * 3 {
        let new_cap = if (*idx).cap == 0 {
            256
        } else {
            match (*idx).cap.checked_mul(2) {
                Some(c) => c,
                None => return false,
            }
        };
        let new_buckets =
            mkr_callocarray(new_cap, core::mem::size_of::<OrderBucket>()) as *mut OrderBucket;
        if new_buckets.is_null() {
            return false;
        }
        let (old_buckets, old_cap) = ((*idx).buckets, (*idx).cap);
        (*idx).buckets = new_buckets;
        (*idx).cap = new_cap;
        (*idx).count = 0;
        for i in 0..old_cap {
            let b = &*old_buckets.add(i);
            if !b.node.is_null() {
                let mask = new_cap - 1;
                let mut j = (ptr_hash(b.node) as usize) & mask;
                while !(*(*idx).buckets.add(j)).node.is_null() {
                    j = (j + 1) & mask;
                }
                *(*idx).buckets.add(j) = OrderBucket { node: b.node, ord: b.ord };
                (*idx).count += 1;
            }
        }
        if !old_buckets.is_null() {
            free_c(old_buckets as *mut c_void);
        }
    }
    let key = node_key::<D>(node);
    let mask = (*idx).cap - 1;
    let mut j = (ptr_hash(key) as usize) & mask;
    loop {
        let slot = (*idx).buckets.add(j);
        if (*slot).node.is_null() {
            *slot = OrderBucket { node: key, ord };
            (*idx).count += 1;
            return true;
        }
        if (*slot).node == key {
            return true; /* already present */
        }
        j = (j + 1) & mask;
    }
}

unsafe fn order_index_lookup<D: Dom>(idx: *const OrderIndex, node: D::Node) -> Option<usize> {
    if (*idx).cap == 0 {
        return None;
    }
    let key = node_key::<D>(node);
    let mask = (*idx).cap - 1;
    let mut j = (ptr_hash(key) as usize) & mask;
    loop {
        let slot = &*(*idx).buckets.add(j);
        if slot.node.is_null() {
            return None;
        }
        if slot.node == key {
            return Some(slot.ord);
        }
        j = (j + 1) & mask;
    }
}

/// A handle as the void pointer the C-side tables key on.
#[inline]
fn node_key<D: Dom>(n: D::Node) -> *const c_void {
    D::to_void(n) as *const c_void
}

/// Pre-order DFS assigning ordinals: the node, then its attributes (before any
/// child), then its descendants - matching `doc_order_cmp`'s placement.
/// Iterative through parent pointers, so a deep tree cannot overflow the stack,
/// and it stays inside the subtree (it never follows `root`'s next).
unsafe fn order_index_walk<D: Dom>(idx: *mut OrderIndex, root: D::Node) -> bool {
    let mut cur = root;
    let mut ord = 0usize;
    while !D::is_null(cur) {
        if !order_index_insert::<D>(idx, cur, ord) {
            return false;
        }
        ord += 1;
        if D::node_type(cur) == NTYPE_ELEMENT {
            let mut a = D::first_attr(cur);
            while !D::is_null(a) {
                if !order_index_insert::<D>(idx, a, ord) {
                    return false;
                }
                ord += 1;
                a = D::attr_next(a);
            }
        }
        if !D::is_null(D::first_child(cur)) {
            cur = D::first_child(cur);
            continue;
        }
        while cur != root && D::is_null(D::next(cur)) {
            cur = D::parent(cur);
        }
        if cur == root {
            break;
        }
        cur = D::next(cur);
    }
    true
}

unsafe fn order_index_build<D: Dom>(idx: *mut OrderIndex, root: D::Node) -> bool {
    if (*idx).built != 0 {
        return true;
    }
    if D::is_null(root) {
        return false;
    }
    if !order_index_walk::<D>(idx, root) {
        mkr_doc_order_index_clear(idx);
        return false;
    }
    (*idx).built = 1;
    true
}

/// The indexed comparator, falling back to the parent-chain walk on any miss
/// (a synthesised node, or a cross-document compare).
unsafe fn doc_order_cmp_ctx<D: Dom>(ctx: *mut Context, a: D::Node, b: D::Node) -> i32 {
    if a == b {
        return 0;
    }
    if ctx.is_null() {
        return doc_order_cmp::<D>(a, b);
    }
    let idx = mkr_ctx_order_index(ctx);
    if idx.is_null() || (*idx).built == 0 {
        return doc_order_cmp::<D>(a, b);
    }
    match (order_index_lookup::<D>(idx, a), order_index_lookup::<D>(idx, b)) {
        (Some(oa), Some(ob)) => oa.cmp(&ob) as i32,
        _ => doc_order_cmp::<D>(a, b),
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
pub unsafe fn nodeset_sort_doc_order<D: Dom>(ctx: *mut Context, ns: *mut NodeSet) {
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
    let cmp = |a: &*mut c_void, b: &*mut c_void| {
        doc_order_cmp_ctx::<D>(ctx, D::from_void(*a), D::from_void(*b))
    };
    if items.windows(2).all(|w| cmp(&w[0], &w[1]) <= 0) {
        return;
    }

    /* Build the index lazily, and only when the sort is large enough to
     * amortise the full-document walk. */
    let idx = if ctx.is_null() { ptr::null_mut() } else { mkr_ctx_order_index(ctx) };
    if !idx.is_null() && (*idx).built == 0 && items.len() >= INDEX_BUILD_MIN {
        let root = mkr_ctx_document(ctx);
        if !root.is_null() {
            /* Best-effort: on OOM the parent-chain comparator still serves. */
            order_index_build::<D>(idx, D::from_void(root));
        }
    }

    /* A stable merge sort, so ties - possible only for synthesised nodes that
     * are not in the index - keep insertion order. Rust's sort_by is exactly
     * that, and it falls back to an in-place merge if it cannot allocate,
     * which is the C's qsort fallback without the loss of stability. */
    items.sort_by(|x, y| cmp(x, y).cmp(&0));
}

/// Sort into document order and drop duplicates.
///
/// # Safety
/// See `nodeset_sort_doc_order`.
pub unsafe fn nodeset_unique_sorted<D: Dom>(ctx: *mut Context, ns: *mut NodeSet) {
    if ns.is_null() || (*ns).count < 2 {
        return;
    }
    nodeset_sort_doc_order::<D>(ctx, ns);
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

/* ---------- the string-value cache's node-keyed insert ---------- */

/// The cached string-value of `node`, building and caching it on a miss.
///
/// The returned bytes are borrowed: the cache owns them until the evaluate that
/// built them unwinds to its snapshot.
///
/// # Safety
/// `ctx` must be the evaluating context and `node` live.
pub unsafe fn cached_node_text<'a, D: Dom>(
    ctx: *mut Context,
    node: D::Node,
    err: *mut Error,
) -> Option<&'a [u8]> {
    let c = mkr_ctx_str_cache(ctx);
    if c.is_null() {
        err_setf!(err, XP_ERR_INTERNAL, "cached_node_text called without a context");
        return None;
    }
    let key = node_key::<D>(node);

    /* O(1) lookup through the pointer-keyed index. */
    if (*c).bucket_cap != 0 {
        let mask = (*c).bucket_cap - 1;
        let mut j = (ptr_hash(key) as usize) & mask;
        while *(*c).buckets.add(j) != 0 {
            let e = &*(*c).entries.add(*(*c).buckets.add(j) - 1);
            if ptr::eq(e.node, key) {
                return Some(borrow(e.str_, e.len));
            }
            j = (j + 1) & mask;
        }
    }

    let limits = mkr_ctx_limits(ctx);
    let mut text = OwnedText { ptr: ptr::null_mut(), len: 0 };
    if !node_to_owned_text::<D>(node, limits, err, &mut text) {
        return None;
    }

    if mkr_grow_reserve(
        &raw mut (*c).entries as *mut *mut c_void,
        &raw mut (*c).cap,
        (*c).count + 1,
        core::mem::size_of::<StrCacheEntry>(),
    ) != MKR_OK
    {
        mkr_owned_text_clear(&mut text);
        err_setf!(err, XP_ERR_OOM, "out of memory in node string cache");
        return None;
    }

    /* A total cap on the cached bytes, so one evaluate cannot grow the cache
     * without bound. */
    let new_total = match (*c).total_bytes.checked_add(text.len) {
        Some(t) => t,
        None => {
            mkr_owned_text_clear(&mut text);
            err_setf!(err, XP_ERR_OOM, "node string cache size overflow");
            return None;
        }
    };
    if mkr_limit_check_string_bytes(limits, new_total, err) != 0 {
        mkr_owned_text_clear(&mut text);
        return None;
    }

    /* Grow the index FIRST. It rebuilds only from the already-committed
     * entries, so every fallible step happens while the slot at [count] is
     * still untouched, and the entry is committed once nothing can fail - no
     * tentative write to roll back. Load factor stays at or below 1/2. */
    if (*c).bucket_cap == 0 || ((*c).count + 1) * 2 > (*c).bucket_cap {
        let new_bucket_cap = if (*c).bucket_cap == 0 {
            64
        } else {
            match (*c).bucket_cap.checked_mul(2) {
                Some(b) => b,
                None => {
                    mkr_owned_text_clear(&mut text);
                    err_setf!(err, XP_ERR_OOM, "node string cache index overflow");
                    return None;
                }
            }
        };
        if mkr_str_cache_reindex(c, new_bucket_cap) != 0 {
            mkr_owned_text_clear(&mut text);
            err_setf!(err, XP_ERR_OOM, "out of memory indexing node string cache");
            return None;
        }
    }

    /* Commit. mkr_str_cache_index_put reads entries[count].node, so the write
     * has to come first. */
    let slot = (*c).entries.add((*c).count);
    (*slot).node = key as *mut c_void;
    (*slot).str_ = text.ptr;
    (*slot).len = text.len;
    mkr_str_cache_index_put(c, (*c).count);
    (*c).total_bytes += text.len;
    (*c).count += 1;

    Some(borrow(text.ptr, text.len))
}

#[inline]
unsafe fn borrow<'a>(p: *const c_char, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p as *const u8, len)
    }
}

extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
