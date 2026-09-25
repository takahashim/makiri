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

use core::ffi::c_void;
use core::ptr::NonNull;

pub mod verify;

/// Why a buffer operation failed. Every failure leaves the buffer as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufError {
    /// Allocation failed, or a size computation overflowed.
    Oom,
    /// The content would pass the buffer's ceiling.
    Limit,
}

/// A growable buffer, NUL-terminated whenever it holds an allocation.
pub struct Buf {
    data: *mut u8,
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

    /// A fresh allocation holding a copy of `bytes`, NUL-terminated, or None on
    /// OOM.
    pub fn copy_from(bytes: &[u8]) -> Option<OwnedBuf> {
        let len = bytes.len();
        // SAFETY: `str_alloc` returns null or `len + 1` writable bytes from libc,
        // which `Drop` frees; the copy and the terminator stay inside them.
        unsafe {
            let p = NonNull::new(crate::falloc::cstr::str_alloc(len) as *mut u8)?;
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), p.as_ptr(), len);
            Some(OwnedBuf { ptr: p, len }) /* `str_alloc` wrote the NUL */
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
        core::slice::from_raw_parts(self.data, self.len)
    }

    /// Append `bytes`. Fails closed, leaving the buffer untouched (see
    /// [`BufError`]).
    pub fn append(&mut self, bytes: &[u8]) -> Result<(), BufError> {
        let n = bytes.len();
        if n == 0 {
            return Ok(());
        }
        let need = self.len.checked_add(n).ok_or(BufError::Oom)?;
        let limit = self.content_limit();
        if need > limit {
            return Err(BufError::Limit);
        }
        /* Room for the NUL terminator too. */
        let need_term = need.checked_add(1).ok_or(BufError::Oom)?;

        if need_term > self.cap {
            let mut new_cap =
                crate::falloc::grow_capacity(self.cap, need_term, 1).ok_or(BufError::Oom)?;
            /* Geometric growth can overshoot to ~2x need_term; clamp the
             * ALLOCATION to the same ceiling as the content (limit, plus the
             * NUL), so cap never runs to ~2x the hard maximum near the limit.
             * Safe: this append already passed `need <= limit`, so
             * `need_term <= limit + 1` and the clamp can never drop new_cap
             * below what this append needs. A limit + 1 that overflows - only a
             * pathological MKR_BUF_HARD_MAX=SIZE_MAX - skips the clamp. */
            if let Some(ceiling) = limit.checked_add(1) {
                new_cap = new_cap.min(ceiling);
            }
            self.realloc_to(new_cap)?;
        }

        // SAFETY: `data` holds `cap >= need + 1` bytes after the growth above,
        // so the `n` copied bytes and the terminator after them fit; `bytes` is
        // a slice, so it names `n` readable bytes, and it cannot overlap a
        // buffer this one owns.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.data.add(self.len), n);
            *self.data.add(need) = 0; /* keep NUL-terminated */
        }
        self.len = need;
        Ok(())
    }

    /// Pre-allocate capacity for `n` content bytes without changing the current
    /// length, so a known-size fill does not realloc on every geometric step.
    ///
    /// Best-effort: it never grows past the buffer's own ceiling, and a later
    /// append still fails closed if the real output exceeds it.
    pub fn reserve(&mut self, n: usize) -> Result<(), BufError> {
        let n = n.min(self.content_limit());
        let need_term = n.checked_add(1).ok_or(BufError::Oom)?;
        if need_term <= self.cap {
            return Ok(()); /* already have room */
        }
        self.realloc_to(need_term)?;
        // SAFETY: `cap > len` (the allocation holds the old content and its
        // terminator), so byte `len` is inside it.
        unsafe { *self.data.add(self.len) = 0 }; /* keep NUL-terminated */
        Ok(())
    }

    /// Detach the NUL-terminated bytes and reset this buffer to empty.
    ///
    /// An empty buffer yields a freshly owned `""`, so the caller never has to
    /// distinguish "no output" from "failed": `Err` is OOM.
    pub fn steal(&mut self) -> Result<OwnedBuf, BufError> {
        let Some(ptr) = NonNull::new(self.data) else {
            // SAFETY: `str_alloc(0)` returns null or one NUL byte from libc,
            // which the `OwnedBuf` frees.
            let p = unsafe { crate::falloc::cstr::str_alloc(0) } as *mut u8;
            let ptr = NonNull::new(p).ok_or(BufError::Oom)?;
            return Ok(OwnedBuf { ptr, len: 0 });
        };
        let len = self.len;
        /* The allocation now belongs to the `OwnedBuf` alone. */
        self.data = core::ptr::null_mut();
        self.len = 0;
        self.cap = 0;
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

    /// The effective content ceiling: this buffer's own `max` (0 meaning the
    /// default), clamped by the absolute hard maximum.
    ///
    /// One function, because `append` and `reserve` must not drift - a
    /// `reserve` with a larger ceiling than `append` would pre-size past what
    /// any append will accept.
    #[inline]
    fn content_limit(&self) -> usize {
        let soft = if self.max != 0 {
            self.max
        } else {
            BUF_DEFAULT_LIMIT
        };
        soft.min(BUF_HARD_MAX)
    }

    /// Reallocate to exactly `cap` bytes. `Err` leaves the buffer as it was.
    ///
    /// The memory is libc's, not Rust's: `steal` hands the pointer to an
    /// `OwnedBuf` that `free()`s it. So this uses `realloc` directly rather
    /// than `falloc`, and consults `falloc::allocation_should_fail`, so
    /// `rake oom` reaches it while production builds compile that hook to
    /// `false`.
    fn realloc_to(&mut self, cap: usize) -> Result<(), BufError> {
        if crate::falloc::allocation_should_fail() {
            return Err(BufError::Oom);
        }
        // SAFETY: `data` is null or this buffer's own libc allocation; on
        // failure `realloc` leaves it untouched, and it is replaced only on
        // success.
        let p = unsafe { libc_realloc(self.data as *mut c_void, cap) } as *mut u8;
        if p.is_null() {
            return Err(BufError::Oom);
        }
        self.data = p;
        self.cap = cap;
        Ok(())
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
    #[link_name = "realloc"]
    fn libc_realloc(p: *mut c_void, n: usize) -> *mut c_void;
}

/* The build-time content limits, each overridable from the environment at
 * build time (`MKR_BUF_HARD_MAX=<bytes>`, `MKR_BUF_DEFAULT_LIMIT=<bytes>`; the
 * variable names predate the port and are kept so an existing build setting
 * still applies). Read in one place, `Buf::content_limit`. */

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
