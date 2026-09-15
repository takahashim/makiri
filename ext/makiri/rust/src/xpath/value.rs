//! The per-backend value model (mkr_xpath_value_body.h): node string-values
//! (XPath 1.0 §5), the coercions that read a node-set's first node, document
//! order, and the string-value cache's node-keyed insert.
//!
//! Generic over `Dom`, where the C compiled the same body once per
//! representation. The values themselves (`Val`, `NodeSet`) keep their C
//! layout: they cross into the glue's custom-function bridge and out as the
//! evaluate result.

use super::abi::*;
use super::dom::*;
use super::number;
use super::own::{OwnedText, OwnedVal};
use crate::err_setf;
use crate::falloc::raw::mkr_reallocarray;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

/// The dynamic context of XPath 1.0 - the "focus": the context node with its
/// 1-based position and the context size. These three always travel together.
///
/// The evaluator is what establishes a focus (a step's per-context predicate
/// pass, and the outermost evaluate); the function library only ever receives
/// one, so it lives here with the other runtime values rather than there.
#[derive(Clone, Copy)]
pub struct Focus<D: Dom> {
    pub node: D::Node,
    pub pos: usize,
    pub size: usize,
}

/* mkr_status_t */
pub const ST_OK: c_int = 0;
pub const ST_ERR_OOM: c_int = 1;
pub const ST_ERR_LIMIT: c_int = 2;

/// A borrowed view of a `mkr_owned_text_t`, empty when the pointer is NULL.
///
/// # Safety
/// `t` must name live bytes for `'a`.
pub unsafe fn owned_bytes<'a>(t: TextSlot) -> &'a [u8] {
    t.as_bytes()
}

/// Copy `s` into a fresh owned text, or `Err` with `err` set to `what` on OOM.
///
/// # Safety
/// None beyond `TextSlot::try_copy_bytes`'s.
pub unsafe fn owned_copy(
    s: &[u8],
    err: ErrSink,
    what: &core::ffi::CStr,
) -> Result<OwnedText, Reported> {
    Ok(OwnedText(TextSlot::try_copy_bytes(s, err, Some(what))?))
}

/* ---------- value clone ---------- */

/// Deep-copy `src`. The node-set case copies the pointer array only: the nodes
/// belong to the document, not to the value.
///
/// # Safety
/// `src` must be a valid value.
pub unsafe fn val_clone(src: &Val, err: ErrSink) -> Result<OwnedVal, Reported> {
    let copy = match src.get() {
        ValRef::String(s) => {
            let mut text = owned_copy(owned_bytes(s), err, c"out of memory cloning string value")?;
            Val::string(text.take())
        }
        ValRef::Number(d) => Val::number(d),
        ValRef::Boolean(b) => Val::boolean(b),
        ValRef::NodeSet(ns) => {
            let n = ns.count;
            if n == 0 {
                return Ok(OwnedVal::new());
            }
            let items = mkr_reallocarray(ptr::null_mut(), n, core::mem::size_of::<*mut c_void>())
                as *mut *mut c_void;
            if items.is_null() {
                return Err(err_setf!(err, XP_ERR_OOM, "out of memory cloning node-set"));
            }
            ptr::copy_nonoverlapping(ns.items, items, n);
            Val::nodeset(NodeSet {
                items,
                count: n,
                capacity: n,
            })
        }
    };
    Ok(copy.into())
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
unsafe fn append_text_descendants<D: Dom>(doc: D::Doc, node: D::Node, buf: *mut Buf) -> c_int {
    let mut cur = D::first_child(doc, node);
    while !D::is_null(cur) {
        let t = D::node_type(doc, cur);
        if t == NTYPE_TEXT || t == NTYPE_CDATA_SECTION {
            let st = D::append_own_text(doc, cur, buf);
            if st != ST_OK {
                return st; /* LIMIT or OOM - the caller fails closed */
            }
        }
        if t == NTYPE_ELEMENT && !D::is_null(D::first_child(doc, cur)) {
            cur = D::first_child(doc, cur);
            continue;
        }
        while cur != node && D::is_null(D::next(doc, cur)) {
            cur = D::parent(doc, cur);
        }
        if cur == node {
            return ST_OK;
        }
        cur = D::next(doc, cur);
    }
    ST_OK
}

unsafe fn build_string_value<D: Dom>(doc: D::Doc, node: D::Node, buf: *mut Buf) -> c_int {
    if D::is_null(node) {
        return ST_OK;
    }
    match D::node_type(doc, node) {
        NTYPE_ATTRIBUTE => {
            let v = D::attr_value(doc, node);
            if v.is_empty() {
                ST_OK
            } else {
                mkr_buf_append(buf, v.as_ptr() as *const c_void, v.len())
            }
        }
        NTYPE_TEXT | NTYPE_CDATA_SECTION | NTYPE_COMMENT | NTYPE_PI => {
            D::append_own_text(doc, node, buf)
        }
        _ => append_text_descendants::<D>(doc, node, buf),
    }
}

/// Build `node`'s XPath string-value - the one node string-value builder.
///
/// With a reporting `err` the build is bounded by `limits.max_string_bytes` and
/// any failure returns `Err` with the slot set. With a silent one it is
/// best-effort: a failure yields an owned "" and returns `Ok`, because the sole
/// such caller is the NUMBER coercion, and a node whose text overran the ceiling
/// was never a valid number - "" coerces to NaN, which is the right answer
/// anyway.
///
/// # Safety
/// `node` must be live.
pub unsafe fn node_to_owned_text<D: Dom>(
    doc: D::Doc,
    node: D::Node,
    limits: *mut Limits,
    err: ErrSink,
) -> Result<OwnedText, Reported> {
    let mut buf = Buf::new(if limits.is_null() {
        0
    } else {
        (*limits).max_string_bytes
    });
    let st = build_string_value::<D>(doc, node, &mut buf);
    if st == ST_OK {
        if let Ok(owned) = buf.steal() {
            return Ok(OwnedText(TextSlot::from_buf(owned)));
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
                    (*limits).max_string_bytes
                )
            } else {
                err_setf!(err, XP_ERR_OOM, "out of memory building node string-value")
            });
        }
    }
    /* best-effort: never fail - yield an owned "". An OOM here leaves the text
     * absent, which reads as "" too. */
    Ok(owned_copy(b"", ErrSink::silent(), c"").unwrap_or_default())
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
pub unsafe fn val_to_number_unchecked<D: Dom>(doc: D::Doc, v: *const Val) -> f64 {
    match (*v).get() {
        ValRef::Number(d) => d,
        ValRef::Boolean(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        ValRef::String(s) => bytes_to_number(owned_bytes(s)),
        ValRef::NodeSet(ns) => {
            if ns.count == 0 {
                return f64::NAN;
            }
            /* string-value of the first node in document order */
            let text = node_text_best_effort::<D>(doc, nodeset_at::<D>(ns, 0));
            bytes_to_number(text.as_slice())
        }
    }
}

/// # Safety
/// `v` must be a valid value whose node pointers are live.
pub unsafe fn val_to_boolean(v: *const Val) -> bool {
    match (*v).get() {
        ValRef::Boolean(b) => b,
        ValRef::Number(d) => !(d == 0.0 || d.is_nan()),
        ValRef::String(s) => s.is_present() && *s.as_ptr() != 0,
        ValRef::NodeSet(ns) => ns.count > 0,
    }
}

/// value -> string (§4.2), bounded by `limits` when it is non-null.
///
/// # Safety
/// `v` may be null (yields "").
pub unsafe fn val_to_owned_text_or_fail<D: Dom>(
    doc: D::Doc,
    v: *const Val,
    limits: *mut Limits,
    err: ErrSink,
) -> Result<OwnedText, Reported> {
    if v.is_null() {
        return owned_copy(b"", err, c"out of memory converting value to string");
    }
    match (*v).get() {
        ValRef::String(s) => {
            let text = owned_bytes(s);
            if !limits.is_null() {
                mkr_limit_check_string_bytes(limits, text.len(), err)?;
            }
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
            if ns.count == 0 {
                return owned_copy(b"", err, c"out of memory");
            }
            /* §4.2: string(node-set) is the string-value of its first node in
             * document order. */
            node_to_owned_text::<D>(doc, nodeset_at::<D>(ns, 0), limits, err)
        }
    }
}

/// value -> number, bounded. Only the node-set case can fail (it builds a
/// string-value first).
///
/// # Safety
/// `v` must be valid.
pub unsafe fn val_to_number_or_fail<D: Dom>(
    doc: D::Doc,
    v: *const Val,
    limits: *mut Limits,
    err: ErrSink,
) -> Result<f64, Reported> {
    if let Some(ns) = (*v).as_nodeset() {
        if ns.count == 0 {
            return Ok(f64::NAN);
        }
        let text = node_to_owned_text::<D>(doc, nodeset_at::<D>(ns, 0), limits, err)?;
        return Ok(bytes_to_number(text.as_slice()));
    }
    Ok(val_to_number_unchecked::<D>(doc, v))
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
unsafe fn node_text_best_effort<D: Dom>(doc: D::Doc, node: D::Node) -> OwnedText {
    node_to_owned_text::<D>(doc, node, ptr::null_mut(), ErrSink::silent()).unwrap_or_default()
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
    err: ErrSink,
) -> Result<&'a [u8], Reported> {
    let doc = D::doc_from_void(mkr_ctx_document(ctx));
    let c = mkr_ctx_str_cache(ctx);
    if c.is_null() {
        return Err(err_setf!(
            err,
            XP_ERR_INTERNAL,
            "cached_node_text called without a context"
        ));
    }
    let key = D::to_void(node) as *const c_void;

    /* O(1) lookup through the pointer-keyed index. */
    if (*c).bucket_cap != 0 {
        let mask = (*c).bucket_cap - 1;
        let mut j = (ptr_hash(key) as usize) & mask;
        while *(*c).buckets.add(j) != 0 {
            let e = &*(*c).entries.add(*(*c).buckets.add(j) - 1);
            if ptr::eq(e.node, key) {
                return Ok(borrow(e.str_, e.len));
            }
            j = (j + 1) & mask;
        }
    }

    let limits = mkr_ctx_limits(ctx);
    /* Held in its guard until the cache takes it, so every refusal below frees
     * it on the way out. */
    let mut text = node_to_owned_text::<D>(doc, node, limits, err)?;

    if mkr_grow_reserve(
        &raw mut (*c).entries as *mut *mut c_void,
        &raw mut (*c).cap,
        (*c).count + 1,
        core::mem::size_of::<StrCacheEntry>(),
    ) != MKR_OK
    {
        return Err(err_setf!(
            err,
            XP_ERR_OOM,
            "out of memory in node string cache"
        ));
    }

    /* A total cap on the cached bytes, so one evaluate cannot grow the cache
     * without bound. */
    let new_total = match (*c).total_bytes.checked_add(text.as_slice().len()) {
        Some(t) => t,
        None => {
            return Err(err_setf!(
                err,
                XP_ERR_OOM,
                "node string cache size overflow"
            ));
        }
    };
    mkr_limit_check_string_bytes(limits, new_total, err)?;

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
                    return Err(err_setf!(
                        err,
                        XP_ERR_OOM,
                        "node string cache index overflow"
                    ));
                }
            }
        };
        if mkr_str_cache_reindex(c, new_bucket_cap) != 0 {
            return Err(err_setf!(
                err,
                XP_ERR_OOM,
                "out of memory indexing node string cache"
            ));
        }
    }

    /* The cache owns the bytes from here. */
    let text = text.take();

    /* Commit. mkr_str_cache_index_put reads entries[count].node, so the write
     * has to come first. */
    let slot = (*c).entries.add((*c).count);
    (*slot).node = key as *mut c_void;
    (*slot).str_ = text.as_ptr();
    (*slot).len = text.len();
    mkr_str_cache_index_put(c, (*c).count);
    (*c).total_bytes += text.len();
    (*c).count += 1;

    Ok(borrow(text.as_ptr(), text.len()))
}

#[inline]
unsafe fn borrow<'a>(p: *const c_char, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(p as *const u8, len)
    }
}
