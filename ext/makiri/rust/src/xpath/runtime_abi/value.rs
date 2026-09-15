//! Runtime XPath values.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use super::{nodeset, text};
use crate::err_setf;

pub unsafe fn val_clear(v: *mut Val) {
    if v.is_null() {
        return;
    }
    match core::mem::replace(&mut *v, Val::EMPTY).get() {
        ValRef::NodeSet(ns) => nodeset::nodeset_clear(&mut { *ns }),
        ValRef::String(mut s) => text::owned_text_clear(&mut s),
        _ => {}
    }
}
pub unsafe fn val_set_owned_text(v: *mut Val, owned: TextSlot) {
    if !v.is_null() {
        *v = Val::string(owned);
    }
}
pub unsafe fn val_set_borrowed_text_copy(
    v: *mut Val,
    borrowed: BorrowedText,
    err: ErrSink,
    what: Option<&core::ffi::CStr>,
) -> core::ffi::c_int {
    if v.is_null() {
        err_setf!(err, XP_ERR_INTERNAL, "val_set_borrowed_text_copy: bad args");
        return -1;
    }
    let Ok(owned) = TextSlot::try_copy(borrowed, err, what) else {
        return -1;
    };
    val_set_owned_text(v, owned);
    0
}
