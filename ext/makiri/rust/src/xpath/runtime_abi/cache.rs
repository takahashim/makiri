//! Per-evaluation order and string-cache storage.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::falloc::raw::mkr_callocarray;
use crate::xpath_abi::ptr_hash;
use core::ffi::c_void;
use core::ptr;

pub unsafe fn doc_order_index_init(idx: *mut OrderIndex) {
    *idx = OrderIndex {
        buckets: ptr::null_mut(),
        cap: 0,
        count: 0,
        built: 0,
    };
}
pub unsafe fn doc_order_index_clear(idx: *mut OrderIndex) {
    if idx.is_null() {
        return;
    }
    if !(*idx).buckets.is_null() {
        free_c((*idx).buckets as *mut c_void);
    }
    doc_order_index_init(idx);
}
pub unsafe fn str_cache_init(c: *mut StrCache) {
    *c = StrCache {
        entries: ptr::null_mut(),
        count: 0,
        cap: 0,
        buckets: ptr::null_mut(),
        bucket_cap: 0,
        total_bytes: 0,
    };
}
pub unsafe fn str_cache_index_put(c: *mut StrCache, idx: usize) {
    let mask = (*c).bucket_cap - 1;
    let mut j = (ptr_hash((*(*c).entries.add(idx)).node as *const c_void) as usize) & mask;
    while *(*c).buckets.add(j) != 0 {
        j = (j + 1) & mask;
    }
    *(*c).buckets.add(j) = idx + 1;
}
pub unsafe fn str_cache_reindex(c: *mut StrCache, bucket_cap: usize) -> i32 {
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
        str_cache_index_put(c, i);
    }
    0
}
pub unsafe fn str_cache_truncate(c: *mut StrCache, target_count: usize) {
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
    if !(*c).buckets.is_null() {
        if target_count == 0 {
            ptr::write_bytes((*c).buckets, 0, (*c).bucket_cap);
            (*c).total_bytes = 0;
        } else {
            str_cache_reindex(c, (*c).bucket_cap);
        }
    }
}
pub unsafe fn str_cache_clear(c: *mut StrCache) {
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
    str_cache_init(c);
}
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
