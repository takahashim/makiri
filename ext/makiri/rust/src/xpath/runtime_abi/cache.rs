//! Per-evaluation order and string-cache storage.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::falloc::raw::callocarray;
use core::ffi::{c_char, c_void};
use core::ptr;

pub struct StrCacheEntry {
    pub node: *mut c_void,
    pub str_: *mut c_char,
    pub len: usize,
}

/// `mkr_str_cache_t` - the per-evaluate node string-value cache: an ordered
/// store plus a pointer-keyed open-addressing index into it.
pub struct StrCache {
    pub entries: *mut StrCacheEntry,
    pub count: usize,
    pub cap: usize,
    /// node pointer -> entry index + 1; 0 is an empty slot.
    pub buckets: *mut usize,
    pub bucket_cap: usize,
    pub total_bytes: usize,
}

/// The MurmurHash3 fmix64 finalizer over a pointer value.
///
/// One definition for every pointer-keyed table: the string-value cache's index
/// is filled by `str_cache_index_put` and probed by its readers, and the
/// text index uses it too, so all of them must hash the same way.
#[inline]
pub fn ptr_hash<T>(p: *const T) -> u64 {
    let mut h = p as usize as u64;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    h
}

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
    let buckets = callocarray(bucket_cap, core::mem::size_of::<usize>()) as *mut usize;
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
