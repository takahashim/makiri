//! Node-set storage and lifecycle.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::err_setf;
use core::ffi::c_void;
use core::ptr;

pub unsafe fn nodeset_init(ns: *mut NodeSet) {
    *ns = NodeSet {
        items: ptr::null_mut(),
        count: 0,
        capacity: 0,
    };
}
pub unsafe fn nodeset_push(
    ns: *mut NodeSet,
    node: *mut c_void,
    budget: *mut Budget,
) -> Result<(), Reported> {
    let err = budget_sink(budget);
    if node.is_null() {
        return Ok(());
    }
    if !budget.is_null() {
        limit_check_nodeset_size(budget, (*ns).count + 1)?;
    }
    if grow_reserve(
        &raw mut (*ns).items as *mut *mut c_void,
        &raw mut (*ns).capacity,
        (*ns).count + 1,
        core::mem::size_of::<*mut c_void>(),
    ) != MKR_OK
    {
        return Err(err_setf!(err, XP_ERR_OOM, "out of memory growing node-set"));
    }
    *(*ns).items.add((*ns).count) = node;
    (*ns).count += 1;
    Ok(())
}
pub unsafe fn nodeset_clear(ns: *mut NodeSet) {
    if ns.is_null() {
        return;
    }
    if !(*ns).items.is_null() {
        free_c((*ns).items as *mut c_void);
    }
    nodeset_init(ns);
}
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
