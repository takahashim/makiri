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

pub mod verify;

/* `mkr_status_t`. Generated would be better, but these five are the C enum's
 * whole content and it has no `-D` override, unlike the limits below. */
/// `MKR_OK` - the `mkr_status_t` the buffer calls return on success.
pub const MKR_OK: c_int = 0;
pub const MKR_ERR_OOM: c_int = 1;
pub const MKR_ERR_LIMIT: c_int = 2;
pub const MKR_ERR_INVALID: c_int = 3;

/// `mkr_buf_t`.
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
        Buf {
            data: core::ptr::null_mut(),
            len: 0,
            cap: 0,
            max,
        }
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
    pub(crate) const mkr_buf_hard_max: usize = match option_env!("MKR_BUF_HARD_MAX") {
        Some(s) => parse_usize(s),
        None => 4 << 30, /* 4 GiB */
    };

    /// The ceiling applied when a buffer was initialised with max == 0. Not
    /// "unbounded" - that is the whole point of having a default.
    pub(crate) const mkr_buf_default_limit: usize = match option_env!("MKR_BUF_DEFAULT_LIMIT") {
        Some(s) => parse_usize(s),
        None => 100 << 20, /* 100 MiB */
    };
}

pub(crate) use limits::{mkr_buf_default_limit, mkr_buf_hard_max};

/* ------------------------------------------------------------------ *
 * the C ABI (core/mkr_buf.c)                                         *
 * ------------------------------------------------------------------ *
 *
 * The buffer's memory is libc's, not Rust's: `mkr_buf_steal` hands the pointer
 * to a caller that `free()`s it, and C code still appends to buffers Rust made.
 * So these use malloc/realloc directly rather than `falloc`, and consult the
 * injection counter themselves - `falloc::should_fail` IS that counter, so
 * `rake oom` reaches these allocations exactly as it reached the C's. */

/// The effective content ceiling for a buffer: its own `max` (0 meaning the
/// default), clamped by the absolute hard maximum.
///
/// One function, because the C computed it identically in `append` and
/// `reserve` and the two must not drift - a `reserve` with a larger ceiling
/// than `append` would pre-size past what any append will accept.
#[inline]
unsafe fn content_limit(b: &Buf) -> usize {
    let soft = if b.max != 0 {
        b.max
    } else {
        mkr_buf_default_limit
    };
    soft.min(mkr_buf_hard_max)
}

/// Append `n` bytes. Fails closed, leaving the buffer untouched:
/// `MKR_ERR_INVALID` for a non-empty write with no source, `MKR_ERR_LIMIT` past
/// the ceiling, `MKR_ERR_OOM` on overflow or allocation failure.
///
/// # Safety
/// `b` must be a live buffer; `bytes` must name `n` readable bytes.
pub unsafe fn mkr_buf_append(b: *mut Buf, bytes: *const c_void, n: usize) -> c_int {
    if n == 0 {
        return MKR_OK;
    }
    if bytes.is_null() {
        return MKR_ERR_INVALID; /* fail closed: nonzero length with no source */
    }
    let b = &mut *b;

    let need = match b.len.checked_add(n) {
        Some(v) => v,
        None => return MKR_ERR_OOM,
    };
    let limit = content_limit(b);
    if need > limit {
        return MKR_ERR_LIMIT;
    }
    let need_term = match need.checked_add(1) {
        Some(v) => v, /* room for the NUL terminator too */
        None => return MKR_ERR_OOM,
    };

    if need_term > b.cap {
        let mut new_cap = match crate::falloc::grow_capacity(b.cap, need_term, 1) {
            Some(c) => c,
            None => return MKR_ERR_OOM,
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
        let p = if crate::falloc::should_fail() {
            core::ptr::null_mut()
        } else {
            libc_realloc(b.data as *mut c_void, new_cap)
        };
        if p.is_null() {
            return MKR_ERR_OOM;
        }
        b.data = p as *mut c_char;
        b.cap = new_cap;
    }

    core::ptr::copy_nonoverlapping(bytes as *const u8, b.data.add(b.len) as *mut u8, n);
    b.len += n;
    *b.data.add(b.len) = 0; /* keep NUL-terminated */
    MKR_OK
}

/// Pre-allocate capacity for `n` bytes, so a known-size fill does not realloc on
/// every geometric step.
///
/// Best-effort: it never grows past the buffer's own ceiling, and a later append
/// still fails closed if the real output exceeds it. `len` is never touched.
///
/// # Safety
/// `b` must be a live buffer.
pub unsafe fn mkr_buf_reserve(b: *mut Buf, n: usize) -> c_int {
    let b = &mut *b;
    let n = n.min(content_limit(b));
    let need_term = match n.checked_add(1) {
        Some(v) => v,
        None => return MKR_ERR_OOM,
    };
    if need_term <= b.cap {
        return MKR_OK; /* already have room */
    }
    let p = if crate::falloc::should_fail() {
        core::ptr::null_mut()
    } else {
        libc_realloc(b.data as *mut c_void, need_term)
    };
    if p.is_null() {
        return MKR_ERR_OOM;
    }
    b.data = p as *mut c_char;
    b.cap = need_term;
    *b.data.add(b.len) = 0; /* keep NUL-terminated */
    MKR_OK
}

/// Take ownership of the NUL-terminated bytes; the buffer is reset to empty.
///
/// An empty buffer yields a freshly owned `""` rather than NULL, so the caller
/// never has to distinguish "no output" from "failed". NULL is therefore
/// unambiguous: it is OOM.
///
/// # Safety
/// `b` must be a live buffer; `out_len` must be NULL or writable.
pub unsafe fn mkr_buf_steal(b: *mut Buf, out_len: *mut usize) -> *mut c_char {
    let b = &mut *b;
    if b.data.is_null() {
        let empty = if crate::falloc::should_fail() {
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
