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

    /// Copy a view into a fresh owned slot, or return `None` on OOM. An absent
    /// view yields a present empty string.
    pub(crate) unsafe fn try_copy(
        t: BorrowedText,
        err: *mut Error,
        what: *const c_char,
    ) -> Option<Self> {
        Self::try_copy_bytes(t.as_bytes(), err, what)
    }

    /// Copy `bytes` into a fresh NUL-terminated slot, interior NULs included,
    /// or return `None` on OOM.
    pub(crate) unsafe fn try_copy_bytes(
        bytes: &[u8],
        err: *mut Error,
        what: *const c_char,
    ) -> Option<Self> {
        let len = bytes.len();
        let src = if len == 0 {
            c"".as_ptr()
        } else {
            bytes.as_ptr() as *const c_char
        };
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
pub unsafe fn mkr_borrowed_text_eq(a: BorrowedText, b: BorrowedText) -> c_int {
    if a.len() != b.len() {
        return 0;
    }
    if a.is_empty() {
        return 1;
    }
    let (x, y) = (a.as_bytes(), b.as_bytes());
    c_int::from(x == y)
}
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
