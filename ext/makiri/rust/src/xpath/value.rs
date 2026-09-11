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
            let st = D::append_own_text(cur, buf);
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
            D::append_own_text(node, buf)
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
            match number::to_text(d, &mut buf) {
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

/// Build `node`'s string-value with no limit and no error reporting - the
/// best-effort form the NUMBER coercion wants, where an overrun yields "" and
/// "" coerces to NaN, which is the right answer anyway.
#[inline]
unsafe fn node_text_best_effort<D: Dom>(node: D::Node) -> Text {
    let mut t = Text::new();
    node_to_owned_text::<D>(node, ptr::null_mut(), ptr::null_mut(), t.as_mut());
    t
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
    let key = D::to_void(node) as *const c_void;

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
