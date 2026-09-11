//! `mkr_buf_t` (core/mkr_buf.h): the owned, capped, growable byte buffer that
//! every C layer collects output into.
//!
//! Declared here rather than inside one subsystem because more than one of them
//! now writes into it - the XPath engine's string values and the glue's
//! serializers - and two Rust copies of one C layout is exactly the drift the
//! layout cross-checks exist to catch.
//!
//! `mkr_buf_init` and `mkr_buf_free` are `static inline` in the header, so they
//! have no symbol to call and are written out as methods; everything else is
//! the exported C function.

use core::ffi::{c_char, c_int, c_void};

/// `MKR_OK` - the `mkr_status_t` the buffer calls return on success.
pub const MKR_OK: c_int = 0;

/// `mkr_buf_t`.
#[repr(C)]
pub struct Buf {
    pub data: *mut c_char,
    pub len: usize,
    pub cap: usize,
    /// 0 selects the conservative default ceiling; it is not "unbounded".
    pub max: usize,
}

impl Buf {
    /// An empty buffer with the soft ceiling `max` (0 = the C default limit).
    ///
    /// The value is not clamped here: every growth path in `mkr_buf.c` takes
    /// `min(max, MKR_BUF_HARD_MAX)`, so passing a larger soft ceiling is
    /// already equivalent to passing the hard one. Restating `MKR_BUF_HARD_MAX`
    /// in Rust would only add a constant that a `-DMKR_BUF_HARD_MAX=` build
    /// could silently disagree with.
    pub fn new(max: usize) -> Buf {
        Buf { data: core::ptr::null_mut(), len: 0, cap: 0, max }
    }

    /// The bytes written so far.
    ///
    /// # Safety
    /// The buffer must not have been freed or stolen from.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.data.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.data as *const u8, self.len)
    }

    /// # Safety
    /// Must not be called twice on the same buffer, or after `mkr_buf_steal`.
    pub unsafe fn free(&mut self) {
        if !self.data.is_null() {
            libc_free(self.data as *mut c_void);
            self.data = core::ptr::null_mut();
        }
        self.len = 0;
        self.cap = 0;
    }
}

extern "C" {
    pub fn mkr_buf_append(b: *mut Buf, bytes: *const c_void, n: usize) -> c_int;
    pub fn mkr_buf_reserve(b: *mut Buf, n: usize) -> c_int;
    pub fn mkr_buf_steal(b: *mut Buf, out_len: *mut usize) -> *mut c_char;

    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}
