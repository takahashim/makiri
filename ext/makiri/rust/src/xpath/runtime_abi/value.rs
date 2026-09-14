//! Runtime XPath values.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use super::{nodeset, text};
use crate::err_setf;
use core::ptr;

pub unsafe fn mkr_val_clear(v: *mut Val) {
    if v.is_null() {
        return;
    }
    match (*v).type_ {
        0 => nodeset::mkr_nodeset_clear(&raw mut (*v).u.nodeset),
        1 => text::mkr_owned_text_clear(&raw mut (*v).u.string),
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
pub unsafe fn mkr_val_set_owned_text(v: *mut Val, owned: OwnedText) {
    if !v.is_null() {
        (*v).type_ = 1;
        (*v).u.string = owned;
    }
}
pub unsafe fn mkr_val_set_borrowed_text_copy(
    v: *mut Val,
    borrowed: VerifiedText,
    err: *mut Error,
    what: *const core::ffi::c_char,
) -> core::ffi::c_int {
    if v.is_null() {
        err_setf!(
            err,
            XP_ERR_INTERNAL,
            "mkr_val_set_borrowed_text_copy: bad args"
        );
        return -1;
    }
    let mut owned = OwnedText::empty();
    if text::mkr_owned_text_from_borrowed_copy(&mut owned, borrowed, err, what) != 0 {
        return -1;
    }
    mkr_val_set_owned_text(v, owned);
    0
}
