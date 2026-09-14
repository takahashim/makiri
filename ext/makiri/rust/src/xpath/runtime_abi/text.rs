//! Owned and borrowed engine text.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::err_setf;
use core::ffi::{c_char, c_int, c_void};

impl OwnedText {
    /// Release the backing allocation and return this slot to the absent state.
    pub(crate) unsafe fn clear(&mut self) {
        if !self.as_ptr().is_null() {
            free_c(self.as_ptr() as *mut c_void);
        }
        *self = Self::empty();
    }

    /// Copy a verified view into a fresh owned slot, or return `None` on OOM.
    pub(crate) unsafe fn try_copy(
        t: VerifiedText,
        err: *mut Error,
        what: *const c_char,
    ) -> Option<Self> {
        let len = if t.ptr.is_null() { 0 } else { t.len };
        let src = if t.ptr.is_null() { c"".as_ptr() } else { t.ptr };
        let p = mkr_strndup(src, len);
        if p.is_null() {
            if what.is_null() {
                err_setf!(err, XP_ERR_OOM, "out of memory copying text");
            } else {
                mkr_err_set(err, XP_ERR_OOM, what);
            }
            return None;
        }
        Some(Self::from_raw_parts(p, len))
    }
}

pub unsafe fn mkr_owned_text_init(t: *mut OwnedText) {
    if !t.is_null() {
        *t = OwnedText::empty();
    }
}
pub unsafe fn mkr_owned_text_clear(t: *mut OwnedText) {
    if t.is_null() {
        return;
    }
    (*t).clear();
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
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
