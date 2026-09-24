//! `Buf`: the owned, capped, growable byte buffer that output is collected
//! into.
//!
//! Declared here rather than inside one subsystem because more than one of them
//! now writes into it - the XPath engine's string values and the glue's
//! serializers.
//!
//! The memory is libc's rather than Rust's allocator, so a stolen buffer is an
//! [`OwnedBuf`] that `free()`s it; see the section on the growth functions.

#![allow(unsafe_code)]

use core::ffi::{c_char, c_void};
use core::ptr::NonNull;

pub mod verify;

/// Why a buffer operation failed. Every failure leaves the buffer as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufError {
    /// Allocation failed, or a size computation overflowed.
    Oom,
    /// The content would pass the buffer's ceiling.
    Limit,
    /// A non-empty write with no source.
    Invalid,
}

/// A growable buffer, NUL-terminated whenever it holds an allocation.
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

// SAFETY: an `OwnedBuf` owns its allocation outright and never writes to it
// after construction - it is `Box<[u8]>` with a NUL - so moving it to another
// thread, or reading it from two, is as sound as for that. The parse reads its
// copy of the source from the GVL-released thread (`bridge::gvl::without_gvl`).
unsafe impl Send for OwnedBuf {}
// SAFETY: as `Send`; `&OwnedBuf` offers only reads.
unsafe impl Sync for OwnedBuf {}

impl OwnedBuf {
    /// The bytes written to the allocation, without the trailing NUL.
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the fields are private and every constructor stores a live
        // libc allocation of at least `len + 1` initialised bytes, owned until
        // `Drop`, which cannot run for `&self`.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl OwnedBuf {
    /// A fresh allocation holding a copy of `bytes`, NUL-terminated, or None on
    /// OOM.
    pub fn copy_from(bytes: &[u8]) -> Option<OwnedBuf> {
        let len = bytes.len();
        // SAFETY: `str_alloc` returns null or `len + 1` writable bytes from libc,
        // which `Drop` frees; the copy and the terminator stay inside them.
        unsafe {
            let p = NonNull::new(crate::falloc::cstr::str_alloc(len) as *mut u8)?;
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), p.as_ptr(), len);
            *p.as_ptr().add(len) = 0;
            Some(OwnedBuf { ptr: p, len })
        }
    }

    /// Room for `cap` bytes, zeroed, that `fill` writes and reports how many it
    /// used; NUL-terminated there. None on OOM.
    pub fn fill(cap: usize, fill: impl FnOnce(&mut [u8]) -> usize) -> Option<OwnedBuf> {
        // SAFETY: as in `copy_from`; the room is zeroed before `fill` sees it.
        let p = unsafe { NonNull::new(crate::falloc::cstr::str_alloc(cap) as *mut u8)? };
        // SAFETY: `p` is `cap + 1` writable bytes nothing else holds yet. This
        // zeroes the first `cap` - so every byte `fill` sees is initialised -
        // and lends exactly those, leaving the terminator byte untouched.
        let dst = unsafe {
            core::ptr::write_bytes(p.as_ptr(), 0, cap);
            core::slice::from_raw_parts_mut(p.as_ptr(), cap)
        };
        let len = fill(dst);
        assert!(len <= cap, "OwnedBuf::fill: wrote past its reservation");
        // SAFETY: `len <= cap`, and byte `cap` exists.
        unsafe { *p.as_ptr().add(len) = 0 };
        Some(OwnedBuf { ptr: p, len })
    }
}

impl Drop for OwnedBuf {
    fn drop(&mut self) {
        // SAFETY: this type owns the libc allocation, and the destructor is the
        // only thing that frees it, so nothing else can still hold it here.
        unsafe { libc_free(self.ptr.as_ptr() as *mut c_void) };
    }
}

impl Buf {
    /// An empty buffer with the soft ceiling `max` (0 = [`BUF_DEFAULT_LIMIT`]).
    ///
    /// The value is not clamped here: every growth path goes through
    /// `content_limit`, which takes `min(max, BUF_HARD_MAX)`, so passing a
    /// larger soft ceiling is already equivalent to passing the hard one.
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
        // SAFETY: the fields are private, and every constructor and mutator in
        // this module leaves `data` either null or an allocation holding `len`
        // initialised bytes - which is exactly what `as_slice_unchecked` needs.
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
        // SAFETY: `self` is a live buffer, and the pointer and length are one
        // Rust slice's, so they name exactly `bytes.len()` readable bytes.
        unsafe { buf_append(self, bytes.as_ptr() as *const c_void, bytes.len()) }
    }

    /// Reserve room for `n` content bytes without changing the current length.
    pub fn reserve(&mut self, n: usize) -> Result<(), BufError> {
        // SAFETY: `self` is a live buffer.
        unsafe { buf_reserve(self, n) }
    }

    /// Detach the allocation and reset this buffer to an empty state.
    pub fn steal(&mut self) -> Result<OwnedBuf, BufError> {
        let mut len = 0usize;
        // SAFETY: `self` is a live buffer and `len` is a writable local. The
        // call resets the buffer to empty, so the allocation it returns has no
        // other owner.
        let ptr = unsafe { buf_steal(self, &mut len) };
        let ptr = NonNull::new(ptr as *mut u8).ok_or(BufError::Oom)?;
        Ok(OwnedBuf { ptr, len })
    }

    /// Release the allocation and reset the buffer to an empty state.
    pub fn free(&mut self) {
        if !self.data.is_null() {
            // SAFETY: non-null here, and owned by this buffer - `steal` is the
            // only way out and it nulls the field. Nulled again below, so a
            // second `free` (or the `Drop` that calls this) cannot double-free.
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

/* The build-time content limits, each overridable from the environment at
 * build time (`MKR_BUF_HARD_MAX=<bytes>`, `MKR_BUF_DEFAULT_LIMIT=<bytes>`; the
 * variable names predate the port and are kept so an existing build setting
 * still applies). Read in one place, `content_limit`. */

mod limits {
    use crate::kani_bounds::parse_usize;

    /// The absolute ceiling on a buffer's CONTENT length.
    pub(crate) const BUF_HARD_MAX: usize = match option_env!("MKR_BUF_HARD_MAX") {
        Some(s) => parse_usize(s),
        None => 4 << 30, /* 4 GiB */
    };

    /// The ceiling applied when a buffer was initialised with max == 0. Not
    /// "unbounded" - that is the whole point of having a default.
    pub(crate) const BUF_DEFAULT_LIMIT: usize = match option_env!("MKR_BUF_DEFAULT_LIMIT") {
        Some(s) => parse_usize(s),
        None => 100 << 20, /* 100 MiB */
    };
}

pub(crate) use limits::{BUF_DEFAULT_LIMIT, BUF_HARD_MAX};

/* ------------------------------------------------------------------ *
 * growth                                                             *
 * ------------------------------------------------------------------ *
 *
 * The buffer's memory is libc's, not Rust's: `buf_steal` hands the pointer to
 * an `OwnedBuf` that `free()`s it. So these use malloc/realloc directly rather
 * than `falloc`, and consult the allocation instrumentation through
 * `falloc::allocation_should_fail`, so `rake oom` reaches them while production
 * builds compile that hook to `false`. */

/// The effective content ceiling for a buffer: its own `max` (0 meaning the
/// default), clamped by the absolute hard maximum.
///
/// One function, because `append` and `reserve` must not drift - a `reserve` with a larger ceiling
/// than `append` would pre-size past what any append will accept.
#[inline]
fn content_limit(b: &Buf) -> usize {
    let soft = if b.max != 0 { b.max } else { BUF_DEFAULT_LIMIT };
    soft.min(BUF_HARD_MAX)
}

/// Append `n` bytes. Fails closed, leaving the buffer untouched (see
/// [`BufError`]).
///
/// `pub(crate)` for the Lexbor serializer callback (`lexbor::serialize`), the
/// one caller that cannot go through [`Buf::append`]'s slice.
///
/// # Safety
/// `b` must be a live buffer; `bytes` must name `n` readable bytes.
pub(crate) unsafe fn buf_append(
    b: *mut Buf,
    bytes: *const c_void,
    n: usize,
) -> Result<(), BufError> {
    if n == 0 {
        return Ok(());
    }
    if bytes.is_null() {
        return Err(BufError::Invalid); /* fail closed: nonzero length with no source */
    }
    let b = &mut *b;

    let need = match b.len.checked_add(n) {
        Some(v) => v,
        None => return Err(BufError::Oom),
    };
    let limit = content_limit(b);
    if need > limit {
        return Err(BufError::Limit);
    }
    let need_term = match need.checked_add(1) {
        Some(v) => v, /* room for the NUL terminator too */
        None => return Err(BufError::Oom),
    };

    if need_term > b.cap {
        let mut new_cap = match crate::falloc::grow_capacity(b.cap, need_term, 1) {
            Some(c) => c,
            None => return Err(BufError::Oom),
        };
        /* Geometric growth can overshoot to ~2x need_term; clamp the ALLOCATION
         * to the same ceiling as the content (limit, plus the NUL), so cap never
         * runs to ~2x the hard maximum near the limit. Safe: this append already
         * passed `need <= limit`, so `need_term <= limit + 1` and the clamp can
         * never drop new_cap below what this append needs. A limit + 1 that
         * overflows - only a pathological MKR_BUF_HARD_MAX=SIZE_MAX - skips
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
            return Err(BufError::Oom);
        }
        b.data = p as *mut c_char;
        b.cap = new_cap;
    }

    core::ptr::copy_nonoverlapping(bytes as *const u8, b.data.add(b.len) as *mut u8, n);
    b.len += n;
    *b.data.add(b.len) = 0; /* keep NUL-terminated */
    Ok(())
}

/// Pre-allocate capacity for `n` bytes, so a known-size fill does not realloc on
/// every geometric step.
///
/// Best-effort: it never grows past the buffer's own ceiling, and a later append
/// still fails closed if the real output exceeds it. `len` is never touched.
///
/// # Safety
/// `b` must be a live buffer.
unsafe fn buf_reserve(b: *mut Buf, n: usize) -> Result<(), BufError> {
    let b = &mut *b;
    let n = n.min(content_limit(b));
    let need_term = match n.checked_add(1) {
        Some(v) => v,
        None => return Err(BufError::Oom),
    };
    if need_term <= b.cap {
        return Ok(()); /* already have room */
    }
    let p = if crate::falloc::allocation_should_fail() {
        core::ptr::null_mut()
    } else {
        libc_realloc(b.data as *mut c_void, need_term)
    };
    if p.is_null() {
        return Err(BufError::Oom);
    }
    b.data = p as *mut c_char;
    b.cap = need_term;
    *b.data.add(b.len) = 0; /* keep NUL-terminated */
    Ok(())
}

/// Take ownership of the NUL-terminated bytes; the buffer is reset to empty.
///
/// An empty buffer yields a freshly owned `""` rather than NULL, so the caller
/// never has to distinguish "no output" from "failed". NULL is therefore
/// unambiguous: it is OOM.
///
/// # Safety
/// `b` must be a live buffer; `out_len` must be NULL or writable.
unsafe fn buf_steal(b: *mut Buf, out_len: *mut usize) -> *mut c_char {
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
