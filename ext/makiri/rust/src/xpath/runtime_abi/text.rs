//! Owned engine text: allocation, copy and release of an `TextSlot` slot.
#![allow(clippy::missing_safety_doc)]
use super::super::abi::*;
use crate::cbuf::OwnedBuf;
use crate::err_setf;
use crate::xpath::msg::err_set;
use core::ffi::{c_char, c_void, CStr};

impl TextSlot {
    /// Release the backing allocation and return this slot to the absent state.
    pub(crate) unsafe fn clear(&mut self) {
        if !self.as_ptr().is_null() {
            free_c(self.as_ptr() as *mut c_void);
        }
        *self = Self::empty();
    }

    /// Copy a view into a fresh owned slot, or `Err` on OOM. An absent view
    /// yields a present empty string.
    pub(crate) unsafe fn try_copy(
        t: BorrowedText,
        err: *mut Error,
        what: Option<&CStr>,
    ) -> Result<Self, Reported> {
        Self::try_copy_bytes(t.as_bytes(), err, what)
    }

    /// Copy `bytes` into a fresh NUL-terminated slot, interior NULs included,
    /// or `Err` on OOM with `*err` set to `what` (or a generic message).
    pub(crate) unsafe fn try_copy_bytes(
        bytes: &[u8],
        err: *mut Error,
        what: Option<&CStr>,
    ) -> Result<Self, Reported> {
        let len = bytes.len();
        let src = if len == 0 {
            c"".as_ptr()
        } else {
            bytes.as_ptr() as *const c_char
        };
        let p = mkr_strndup(src, len);
        if p.is_null() {
            return Err(match what {
                Some(what) => err_set(err, XP_ERR_OOM, what),
                None => err_setf!(err, XP_ERR_OOM, "out of memory copying text"),
            });
        }
        Ok(Self::from_raw_parts(p, len))
    }

    /// Adopt a buffer detached from a `Buf`: a NUL-terminated libc allocation,
    /// which is what `clear` frees.
    pub(crate) fn from_buf(buf: OwnedBuf) -> Self {
        let (ptr, len) = buf.into_raw_parts();
        // SAFETY: `OwnedBuf` owns `len` bytes and a trailing NUL from libc.
        unsafe { Self::from_raw_parts(ptr as *mut c_char, len) }
    }

    /// Allocate room for `cap` bytes, let `fill` write into them and return how
    /// many it used, then NUL-terminate there. `None` on OOM.
    ///
    /// The room is zeroed first, so `fill` never sees uninitialised bytes.
    pub(crate) fn try_fill(cap: usize, fill: impl FnOnce(&mut [u8]) -> usize) -> Option<Self> {
        // SAFETY: `mkr_str_alloc` returns null or `cap + 1` writable bytes.
        let p = unsafe { crate::falloc::cstr::mkr_str_alloc(cap) };
        if p.is_null() {
            return None;
        }
        let dst = unsafe {
            core::ptr::write_bytes(p, 0, cap);
            core::slice::from_raw_parts_mut(p as *mut u8, cap)
        };
        let len = fill(dst);
        assert!(len <= cap, "try_fill: wrote past its reservation");
        // SAFETY: `len <= cap`, and byte `cap` exists; the allocation is libc's.
        unsafe {
            *p.add(len) = 0;
            Some(Self::from_raw_parts(p, len))
        }
    }
}

pub unsafe fn mkr_owned_text_init(t: *mut TextSlot) {
    if !t.is_null() {
        *t = TextSlot::empty();
    }
}
pub unsafe fn mkr_owned_text_clear(t: *mut TextSlot) {
    if t.is_null() {
        return;
    }
    (*t).clear();
}
extern "C" {
    #[link_name = "free"]
    fn free_c(p: *mut c_void);
}
