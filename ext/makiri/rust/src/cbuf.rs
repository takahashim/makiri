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
use core::ptr::NonNull;

pub mod verify;

/* `mkr_status_t`. Generated would be better, but these five are the C enum's
 * whole content and it has no `-D` override, unlike the limits below. */
/// `BUF_OK` - the `mkr_status_t` the buffer calls return on success.
pub const BUF_OK: c_int = 0;
pub const BUF_ERR_OOM: c_int = 1;
pub const BUF_ERR_LIMIT: c_int = 2;
pub const BUF_ERR_INVALID: c_int = 3;

/// A failure returned by the safe buffer API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufError {
    Oom,
    Limit,
    Invalid,
}

/// `mkr_buf_t`.
pub struct Buf {
    data: *mut c_char,
    len: usize,
    cap: usize,
    /// 0 selects the conservative default ceiling; it is not "unbounded".
    max: usize,
}

/// A NUL-terminated libc allocation detached from a [`Buf`].
pub struct OwnedBuf {
    ptr: NonNull<u8>,
    len: usize,
}

impl OwnedBuf {
    /// The bytes written to the allocation, without the trailing NUL.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// Return the allocation to a caller whose ABI requires a raw pointer.
    ///
    /// Ownership is transferred to the caller, which must free the pointer
    /// with libc `free`. Consuming `self` prevents its destructor from running.
    pub fn into_raw_parts(self) -> (*mut u8, usize) {
        let this = core::mem::ManuallyDrop::new(self);
        (this.ptr.as_ptr(), this.len)
    }
}

impl Drop for OwnedBuf {
    fn drop(&mut self) {
        unsafe { libc_free(self.ptr.as_ptr() as *mut c_void) };
    }
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
        Buf {
            data: core::ptr::null_mut(),
            len: 0,
            cap: 0,
            max,
        }
    }

    /// The bytes written so far, without the trailing NUL.
    pub fn as_slice(&self) -> &[u8] {
        /* The fields are private, and every constructor/mutator in this module
         * preserves the allocation invariant used here. */
        unsafe { self.as_slice_unchecked() }
    }

    #[inline]
    unsafe fn as_slice_unchecked(&self) -> &[u8] {
        if self.data.is_null() || self.len == 0 {
            return &[];
        }
        core::slice::from_raw_parts(self.data as *const u8, self.len)
    }

    /// Append bytes without exposing raw pointers to Rust callers.
    pub fn append(&mut self, bytes: &[u8]) -> Result<(), BufError> {
        let status = unsafe { buf_append(self, bytes.as_ptr() as *const c_void, bytes.len()) };
        match status {
            BUF_OK => Ok(()),
            BUF_ERR_OOM => Err(BufError::Oom),
            BUF_ERR_LIMIT => Err(BufError::Limit),
            BUF_ERR_INVALID => Err(BufError::Invalid),
            _ => Err(BufError::Invalid),
        }
    }

    /// Reserve room for `n` content bytes without changing the current length.
    pub fn reserve(&mut self, n: usize) -> Result<(), BufError> {
        let status = unsafe { buf_reserve(self, n) };
        match status {
            BUF_OK => Ok(()),
            BUF_ERR_OOM => Err(BufError::Oom),
            BUF_ERR_LIMIT => Err(BufError::Limit),
            BUF_ERR_INVALID => Err(BufError::Invalid),
            _ => Err(BufError::Invalid),
        }
    }

    /// Detach the allocation and reset this buffer to an empty state.
    pub fn steal(&mut self) -> Result<OwnedBuf, BufError> {
        let mut len = 0usize;
        let ptr = unsafe { buf_steal(self, &mut len) };
        let ptr = NonNull::new(ptr as *mut u8).ok_or(BufError::Oom)?;
        Ok(OwnedBuf { ptr, len })
    }

    /// Release the allocation and reset the buffer to an empty state.
    pub fn free(&mut self) {
        if !self.data.is_null() {
            unsafe { libc_free(self.data as *mut c_void) };
            self.data = core::ptr::null_mut();
        }
        self.len = 0;
        self.cap = 0;
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        self.free();
    }
}

extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

/* Paired with `free` above: the buffer's memory is libc's, for the reason given
 * at the ABI comment below. */
extern "C" {
    #[link_name = "malloc"]
    fn libc_malloc(n: usize) -> *mut c_void;
    #[link_name = "realloc"]
    fn libc_realloc(p: *mut c_void, n: usize) -> *mut c_void;
}

/* The build-time content limits.
 *
 * Alongside the C these are read FROM it: `core/mkr_core_abi.c` publishes the
 * `-D`-overridable macros of `mkr_buf.h` from a translation unit the
 * preprocessor has already seen, so a `-DMKR_BUF_HARD_MAX=` build cannot end up
 * with two different ceilings in one extension.
 *
 * Standing alone there is no preprocessor to be that source, so these become the
 * definition and the override arrives from the environment instead. That is a
 * CHANGE OF SPELLING for anyone who set one: `-DMKR_BUF_HARD_MAX=<bytes>` at
 * compile time becomes `MKR_BUF_HARD_MAX=<bytes>` in the environment. The
 * defaults are unchanged, and they are still read in one place.
 *
 * The lower-case names are deliberate - they are what the C ABI published, and
 * `content_limit` below should not have to know which side defines them. */

#[allow(non_upper_case_globals)]
mod limits {
    use crate::kani_bounds::parse_usize;

    /// The absolute ceiling on a buffer's CONTENT length.
    pub(crate) const buf_hard_max: usize = match option_env!("MKR_BUF_HARD_MAX") {
        Some(s) => parse_usize(s),
        None => 4 << 30, /* 4 GiB */
    };

    /// The ceiling applied when a buffer was initialised with max == 0. Not
    /// "unbounded" - that is the whole point of having a default.
    pub(crate) const buf_default_limit: usize = match option_env!("MKR_BUF_DEFAULT_LIMIT") {
        Some(s) => parse_usize(s),
        None => 100 << 20, /* 100 MiB */
    };
}

pub(crate) use limits::{buf_default_limit, buf_hard_max};

/* ------------------------------------------------------------------ *
 * the C ABI (core/mkr_buf.c)                                         *
 * ------------------------------------------------------------------ *
 *
 * The buffer's memory is libc's, not Rust's: `buf_steal` hands the pointer
 * to a caller that `free()`s it, and C code still appends to buffers Rust made.
 * So these use malloc/realloc directly rather than `falloc`, and consult the
 * allocation instrumentation through `falloc::allocation_should_fail`, so
 * `rake oom` reaches these allocations exactly as it reached the C's while
 * production builds compile that hook to `false`. */

/// The effective content ceiling for a buffer: its own `max` (0 meaning the
/// default), clamped by the absolute hard maximum.
///
/// One function, because the C computed it identically in `append` and
/// `reserve` and the two must not drift - a `reserve` with a larger ceiling
/// than `append` would pre-size past what any append will accept.
#[inline]
fn content_limit(b: &Buf) -> usize {
    let soft = if b.max != 0 { b.max } else { buf_default_limit };
    soft.min(buf_hard_max)
}

/// Append `n` bytes. Fails closed, leaving the buffer untouched:
/// `BUF_ERR_INVALID` for a non-empty write with no source, `BUF_ERR_LIMIT` past
/// the ceiling, `BUF_ERR_OOM` on overflow or allocation failure.
///
/// # Safety
/// `b` must be a live buffer; `bytes` must name `n` readable bytes.
pub(crate) unsafe fn buf_append(b: *mut Buf, bytes: *const c_void, n: usize) -> c_int {
    if n == 0 {
        return BUF_OK;
    }
    if bytes.is_null() {
        return BUF_ERR_INVALID; /* fail closed: nonzero length with no source */
    }
    let b = &mut *b;

    let need = match b.len.checked_add(n) {
        Some(v) => v,
        None => return BUF_ERR_OOM,
    };
    let limit = content_limit(b);
    if need > limit {
        return BUF_ERR_LIMIT;
    }
    let need_term = match need.checked_add(1) {
        Some(v) => v, /* room for the NUL terminator too */
        None => return BUF_ERR_OOM,
    };

    if need_term > b.cap {
        let mut new_cap = match crate::falloc::grow_capacity(b.cap, need_term, 1) {
            Some(c) => c,
            None => return BUF_ERR_OOM,
        };
        /* Geometric growth can overshoot to ~2x need_term; clamp the ALLOCATION
         * to the same ceiling as the content (limit, plus the NUL), so cap never
         * runs to ~2x the hard maximum near the limit. Safe: this append already
         * passed `need <= limit`, so `need_term <= limit + 1` and the clamp can
         * never drop new_cap below what this append needs. A limit + 1 that
         * overflows - only a pathological -DMKR_BUF_HARD_MAX=SIZE_MAX - skips
         * the clamp. */
        if let Some(ceiling) = limit.checked_add(1) {
            if new_cap > ceiling {
                new_cap = ceiling;
            }
        }
        let p = if crate::falloc::allocation_should_fail() {
            core::ptr::null_mut()
        } else {
            libc_realloc(b.data as *mut c_void, new_cap)
        };
        if p.is_null() {
            return BUF_ERR_OOM;
        }
        b.data = p as *mut c_char;
        b.cap = new_cap;
    }

    core::ptr::copy_nonoverlapping(bytes as *const u8, b.data.add(b.len) as *mut u8, n);
    b.len += n;
    *b.data.add(b.len) = 0; /* keep NUL-terminated */
    BUF_OK
}

/// Pre-allocate capacity for `n` bytes, so a known-size fill does not realloc on
/// every geometric step.
///
/// Best-effort: it never grows past the buffer's own ceiling, and a later append
/// still fails closed if the real output exceeds it. `len` is never touched.
///
/// # Safety
/// `b` must be a live buffer.
pub(crate) unsafe fn buf_reserve(b: *mut Buf, n: usize) -> c_int {
    let b = &mut *b;
    let n = n.min(content_limit(b));
    let need_term = match n.checked_add(1) {
        Some(v) => v,
        None => return BUF_ERR_OOM,
    };
    if need_term <= b.cap {
        return BUF_OK; /* already have room */
    }
    let p = if crate::falloc::allocation_should_fail() {
        core::ptr::null_mut()
    } else {
        libc_realloc(b.data as *mut c_void, need_term)
    };
    if p.is_null() {
        return BUF_ERR_OOM;
    }
    b.data = p as *mut c_char;
    b.cap = need_term;
    *b.data.add(b.len) = 0; /* keep NUL-terminated */
    BUF_OK
}

/// Take ownership of the NUL-terminated bytes; the buffer is reset to empty.
///
/// An empty buffer yields a freshly owned `""` rather than NULL, so the caller
/// never has to distinguish "no output" from "failed". NULL is therefore
/// unambiguous: it is OOM.
///
/// # Safety
/// `b` must be a live buffer; `out_len` must be NULL or writable.
pub(crate) unsafe fn buf_steal(b: *mut Buf, out_len: *mut usize) -> *mut c_char {
    let b = &mut *b;
    if b.data.is_null() {
        let empty = if crate::falloc::allocation_should_fail() {
            core::ptr::null_mut()
        } else {
            libc_malloc(1)
        };
        if empty.is_null() {
            return core::ptr::null_mut();
        }
        *(empty as *mut u8) = 0;
        if !out_len.is_null() {
            *out_len = 0;
        }
        return empty as *mut c_char;
    }
    let p = b.data;
    if !out_len.is_null() {
        *out_len = b.len;
    }
    b.data = core::ptr::null_mut();
    b.len = 0;
    b.cap = 0;
    p
}

#[cfg(test)]
mod tests {
    use super::{Buf, BufError};

    #[test]
    fn safe_api_appends_and_reserves_without_exposing_storage() {
        let mut buf = Buf::new(16);
        assert_eq!(buf.reserve(8), Ok(()));
        assert_eq!(buf.append(b"hello"), Ok(()));
        assert_eq!(buf.as_slice(), b"hello");
        assert_eq!(buf.append(b" world"), Ok(()));
        assert_eq!(buf.as_slice(), b"hello world");
    }

    #[test]
    fn limit_failure_leaves_the_buffer_unchanged() {
        let mut buf = Buf::new(5);
        assert_eq!(buf.append(b"hello"), Ok(()));
        assert_eq!(buf.append(b"!"), Err(BufError::Limit));
        assert_eq!(buf.as_slice(), b"hello");
    }

    #[test]
    fn steal_transfers_ownership_and_resets_the_buffer() {
        let mut buf = Buf::new(16);
        buf.append(b"owned").unwrap();

        let owned = buf.steal().unwrap();
        assert_eq!(owned.as_slice(), b"owned");
        assert!(buf.as_slice().is_empty());
        drop(owned); /* exercises OwnedBuf's libc-free Drop */
    }

    #[test]
    fn dropping_a_live_buffer_releases_it_without_manual_free() {
        let mut buf = Buf::new(16);
        buf.append(b"dropped").unwrap();
        drop(buf); /* exercises Buf's libc-free Drop */
    }
}
