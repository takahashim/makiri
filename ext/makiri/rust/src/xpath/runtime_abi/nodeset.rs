//! Node-set storage and lifecycle.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::err_setf;
use core::ffi::{c_int, c_void};
use core::ptr;

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
    mkr_nodeset_init(ns);
}
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
