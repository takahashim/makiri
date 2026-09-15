//! The raw boundary the glue calls: setting and clearing the errors and results
//! it holds.
//!
//! The evaluator uses the typed APIs in the sibling modules. These stay raw
//! because the glue holds errors and results as C-layout structs.

use crate::falloc::raw::free_and_null;
use crate::xpath_abi::{Error, XPathValue, XP_OK};
use core::ffi::{c_char, c_int, c_void};

const MKR_XPATH_TYPE_NODESET: u32 = 0;
const MKR_XPATH_TYPE_STRING: u32 = 1;

/// Replace an error message, freeing the one the error held.
///
/// # Safety
/// `err` is null or a live error; `msg` is null or NUL-terminated.
pub unsafe extern "C" fn mkr_err_set(err: *mut Error, status: c_int, msg: *const c_char) {
    if err.is_null() {
        return;
    }
    let err = &mut *err;
    free_and_null(err.message as *mut c_void);
    err.status = status;
    err.message = if msg.is_null() {
        core::ptr::null_mut()
    } else {
        crate::falloc::cstr::mkr_strdup(msg)
    };
}

/// Release an error message and reset its status.
///
/// # Safety
/// `e` is null or a live error.
pub unsafe extern "C" fn mkr_xpath_error_clear(e: *mut Error) {
    if e.is_null() {
        return;
    }
    let e = &mut *e;
    free_and_null(e.message as *mut c_void);
    e.message = core::ptr::null_mut();
    e.status = XP_OK;
}

/// Release memory owned by an XPath result.
///
/// # Safety
/// `v` is null or a live value whose `type_` identifies its active arm.
pub unsafe extern "C" fn mkr_xpath_value_clear(v: *mut XPathValue) {
    if v.is_null() {
        return;
    }
    let v = &mut *v;
    match v.type_ {
        MKR_XPATH_TYPE_NODESET => {
            free_and_null(v.u.nodeset.nodes as *mut c_void);
            v.u.nodeset.nodes = core::ptr::null_mut();
            v.u.nodeset.count = 0;
        }
        MKR_XPATH_TYPE_STRING => v.u.string.clear(),
        _ => {}
    }
}
