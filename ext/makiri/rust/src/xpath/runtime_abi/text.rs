//! Owned and borrowed engine text.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::err_setf;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;

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
    mkr_owned_text_init(t);
}
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
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
