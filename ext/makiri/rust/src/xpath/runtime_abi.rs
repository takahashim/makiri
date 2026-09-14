//! The raw runtime ABI primitives formerly provided by `mkr_xpath_shared.c`:
//! node-set build and free, owned engine strings, the runtime value, and the
//! lifecycles of the two per-evaluate caches.
//!
//! None of this dereferences a DOM node. It moves erased node handles, owns and
//! compares engine strings, and manages cache storage. These functions retain
//! raw-pointer signatures because they are also consumed by the glue and CSS
//! lowering; the safe RAII views live in `own.rs`.
//!
//! Every function here is exported: the glue, the CSS lowering, the parser, the
//! driver and both engine instances all call them by name.

/* Each takes pointers its caller already holds; the contract is the one at the
 * declaration in mkr_xpath.h / mkr_xpath_internal.h. */
#![allow(clippy::missing_safety_doc)]

use super::abi::*;
use crate::err_setf;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

/* ---------- node-set ---------- */

pub unsafe fn mkr_nodeset_init(ns: *mut NodeSet) {
    *ns = NodeSet {
        items: ptr::null_mut(),
        count: 0,
        capacity: 0,
    };
}

pub unsafe fn mkr_nodeset_push(
    ns: *mut NodeSet,
    node: *mut c_void,
    limits: *mut Limits,
    err: *mut Error,
) -> c_int {
    if node.is_null() {
        return 0;
    }
    if !limits.is_null() && mkr_limit_check_nodeset_size(limits, (*ns).count + 1, err) != 0 {
        return -1;
    }
    if mkr_grow_reserve(
        &raw mut (*ns).items as *mut *mut c_void,
        &raw mut (*ns).capacity,
        (*ns).count + 1,
        core::mem::size_of::<*mut c_void>(),
    ) != MKR_OK
    {
        err_setf!(err, XP_ERR_OOM, "out of memory growing node-set");
        return -1;
    }
    *(*ns).items.add((*ns).count) = node;
    (*ns).count += 1;
    0
}

pub unsafe fn mkr_nodeset_clear(ns: *mut NodeSet) {
    if ns.is_null() {
        return;
    }
    if !(*ns).items.is_null() {
        free_c((*ns).items as *mut c_void);
    }
    *ns = NodeSet {
        items: ptr::null_mut(),
        count: 0,
        capacity: 0,
    };
}

/* ---------- owned / borrowed text ---------- */

pub unsafe fn mkr_owned_text_init(t: *mut OwnedText) {
    if !t.is_null() {
        *t = OwnedText {
            ptr: ptr::null_mut(),
            len: 0,
        };
    }
}

pub unsafe fn mkr_owned_text_clear(t: *mut OwnedText) {
    if t.is_null() {
        return;
    }
    if !(*t).ptr.is_null() {
        free_c((*t).ptr as *mut c_void);
    }
    *t = OwnedText {
        ptr: ptr::null_mut(),
        len: 0,
    };
}

/// Equal lengths AND equal bytes. A zero-length view is equal regardless of its
/// pointer, so a NULL-represented empty and a ""-represented one compare equal
/// and NULL is never dereferenced.
pub unsafe fn mkr_borrowed_text_eq(a: VerifiedText, b: VerifiedText) -> c_int {
    if a.len != b.len {
        return 0;
    }
    if a.len == 0 {
        return 1;
    }
    let (x, y) = (
        core::slice::from_raw_parts(a.ptr as *const u8, a.len),
        core::slice::from_raw_parts(b.ptr as *const u8, b.len),
    );
    c_int::from(x == y)
}

/// Copy an already-valid borrowed text into owned storage.
///
/// It takes a text view rather than raw bytes and a length to keep the type
/// contract: an owned text can only be minted from text a caller has asserted
/// valid, so every raw-bytes entry point stays greppable.
pub unsafe fn mkr_owned_text_from_borrowed_copy(
    out: *mut OwnedText,
    t: VerifiedText,
    err: *mut Error,
    what: *const c_char,
) -> c_int {
    if out.is_null() {
        err_setf!(
            err,
            XP_ERR_INTERNAL,
            "mkr_owned_text_from_borrowed_copy: bad args"
        );
        return -1;
    }
    mkr_owned_text_init(out);
    let len = if t.ptr.is_null() { 0 } else { t.len };
    let src = if t.ptr.is_null() { c"".as_ptr() } else { t.ptr };
    let p = mkr_strndup(src, len);
    if p.is_null() {
        if what.is_null() {
            err_setf!(err, XP_ERR_OOM, "out of memory copying text");
        } else {
            mkr_err_set(err, XP_ERR_OOM, what);
        }
        return -1;
    }
    (*out).ptr = p;
    (*out).len = len;
    0
}

/* ---------- value ---------- */

pub unsafe fn mkr_val_clear(v: *mut Val) {
    if v.is_null() {
        return;
    }
    match (*v).type_ {
        0 /* nodeset */ => mkr_nodeset_clear(&raw mut (*v).u.nodeset),
        1 /* string */ => mkr_owned_text_clear(&raw mut (*v).u.string),
        _ => {}
    }
    *v = Val {
        type_: 0,
        u: ValU {
            nodeset: NodeSet {
                items: ptr::null_mut(),
                count: 0,
                capacity: 0,
            },
        },
    };
}

pub unsafe fn mkr_val_set_owned_text(v: *mut Val, text: OwnedText) {
    if !v.is_null() {
        (*v).type_ = 1 /* string */;
        (*v).u.string = text;
    }
}

/// Set `v` to a STRING by copying a borrowed view: the engine allocates and
/// owns the copy.
///
/// This is how callers outside the engine - the glue's handler bridge - hand a
/// string into a value: they pass what they have, a borrowed slice, and never
/// construct an owned text themselves. Keeping the copy-and-own step here keeps
/// allocating and freeing owned strings in one layer.
pub unsafe fn mkr_val_set_borrowed_text_copy(
    v: *mut Val,
    text: VerifiedText,
    err: *mut Error,
    what: *const c_char,
) -> c_int {
    if v.is_null() {
        err_setf!(
            err,
            XP_ERR_INTERNAL,
            "mkr_val_set_borrowed_text_copy: bad args"
        );
        return -1;
    }
    let mut owned = OwnedText {
        ptr: ptr::null_mut(),
        len: 0,
    };
    if mkr_owned_text_from_borrowed_copy(&mut owned, text, err, what) != 0 {
        return -1;
    }
    mkr_val_set_owned_text(v, owned);
    0
}

/* ---------- the per-evaluate document-order index (lifecycle) ---------- */

pub unsafe fn mkr_doc_order_index_init(idx: *mut OrderIndex) {
    *idx = OrderIndex {
        buckets: ptr::null_mut(),
        cap: 0,
        count: 0,
        built: 0,
    };
}

pub unsafe fn mkr_doc_order_index_clear(idx: *mut OrderIndex) {
    if idx.is_null() {
        return;
    }
    if !(*idx).buckets.is_null() {
        free_c((*idx).buckets as *mut c_void);
    }
    *idx = OrderIndex {
        buckets: ptr::null_mut(),
        cap: 0,
        count: 0,
        built: 0,
    };
}

/* ---------- the per-evaluate string-value cache (lifecycle + index) ---------- */

pub unsafe fn mkr_str_cache_init(c: *mut StrCache) {
    *c = StrCache {
        entries: ptr::null_mut(),
        count: 0,
        cap: 0,
        buckets: ptr::null_mut(),
        bucket_cap: 0,
        total_bytes: 0,
    };
}

/// Insert entry `idx`, keyed by its node, into the index. The index must have
/// room - callers grow or rehash first.
///
/// Exported rather than private because the cache splits its pure index
/// bookkeeping from its node-dereferencing insert, which lives in the
/// per-backend value module: both drive this one implementation.
pub unsafe fn mkr_str_cache_index_put(c: *mut StrCache, idx: usize) {
    let mask = (*c).bucket_cap - 1;
    let mut j = (ptr_hash((*(*c).entries.add(idx)).node as *const c_void) as usize) & mask;
    while *(*c).buckets.add(j) != 0 {
        j = (j + 1) & mask;
    }
    *(*c).buckets.add(j) = idx + 1;
}

/// Rebuild the index from the committed entries. -1 on OOM.
pub unsafe fn mkr_str_cache_reindex(c: *mut StrCache, bucket_cap: usize) -> c_int {
    let buckets = mkr_callocarray(bucket_cap, core::mem::size_of::<usize>()) as *mut usize;
    if buckets.is_null() {
        return -1;
    }
    if !(*c).buckets.is_null() {
        free_c((*c).buckets as *mut c_void);
    }
    (*c).buckets = buckets;
    (*c).bucket_cap = bucket_cap;
    for i in 0..(*c).count {
        mkr_str_cache_index_put(c, i);
    }
    0
}

pub unsafe fn mkr_str_cache_truncate(c: *mut StrCache, target_count: usize) {
    if c.is_null() || target_count >= (*c).count {
        return;
    }
    for i in target_count..(*c).count {
        let e = &*(*c).entries.add(i);
        (*c).total_bytes = (*c).total_bytes.saturating_sub(e.len);
        if !e.str_.is_null() {
            free_c(e.str_ as *mut c_void);
        }
    }
    (*c).count = target_count;
    /* Drop the removed nodes from the index. A full truncate just clears it; a
     * partial one - the nested-eval snapshot restore - rebuilds from what
     * remains. */
    if !(*c).buckets.is_null() {
        if target_count == 0 {
            ptr::write_bytes((*c).buckets, 0, (*c).bucket_cap);
            (*c).total_bytes = 0;
        } else {
            mkr_str_cache_reindex(c, (*c).bucket_cap);
        }
    }
}

pub unsafe fn mkr_str_cache_clear(c: *mut StrCache) {
    if c.is_null() {
        return;
    }
    for i in 0..(*c).count {
        let e = &*(*c).entries.add(i);
        if !e.str_.is_null() {
            free_c(e.str_ as *mut c_void);
        }
    }
    if !(*c).entries.is_null() {
        free_c((*c).entries as *mut c_void);
    }
    if !(*c).buckets.is_null() {
        free_c((*c).buckets as *mut c_void);
    }
    mkr_str_cache_init(c);
}

extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
