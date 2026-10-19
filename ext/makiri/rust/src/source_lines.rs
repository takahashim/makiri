//! The source line table: byte offset -> 1-based line, for `Node#line`.
//!
//! Built once over the parsed input; the parse stamps each element with its
//! start tag's byte offset, and a lookup is a binary search. Shared by every
//! HTML DOM, so it depends on nothing but the bytes.

#![allow(unsafe_code)]

use core::ffi::c_void;

use crate::falloc::try_vec_with_capacity;

extern "C" {
    /// libc `memchr` - see [`next_newline`].
    #[link_name = "memchr"]
    fn libc_memchr(s: *const c_void, c: core::ffi::c_int, n: usize) -> *const c_void;
}

/* ------------------------------------------------------------------ *
 * line table                                                         *
 * ------------------------------------------------------------------ */

pub struct Lines {
    /// `starts[i]` is the byte offset of line `i + 1`; `starts[0] == 0`.
    starts: Vec<usize>,
}

impl Lines {
    /// The largest line whose start offset is `<= offset`, 1-based.
    ///
    /// `partition_point` is the same binary search the C wrote out: the count of
    /// starts at or below the offset IS the 1-based line.
    pub fn lookup(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }
}

/// The offset of the next newline at or after `from`, or `None`.
///
/// libc `memchr`.
/// A scalar `iter().position()` here measured about 9% off the whole parse on a
/// 220 KB document - the line table walks every input byte twice, so the
/// difference between a vectorised scan and a byte loop is the difference
/// between this subsystem being free and being visible.
#[inline]
fn next_newline(bytes: &[u8], from: usize) -> Option<usize> {
    let rest = bytes.get(from..).filter(|r| !r.is_empty())?;
    // SAFETY: `rest` is `rest.len()` readable bytes.
    let p = unsafe {
        libc_memchr(
            rest.as_ptr() as *const c_void,
            b'\n' as core::ffi::c_int,
            rest.len(),
        )
    };
    if p.is_null() {
        None
    } else {
        Some(p as usize - bytes.as_ptr() as usize)
    }
}

/// Build the line table over the input. None on allocation failure, which the
/// caller treats as "no line information" rather than as a parse failure.
pub fn lines_build(bytes: &[u8]) -> Option<Lines> {
    /* Count first, so the array is sized exactly once. Both passes go through
     * `next_newline`, so they cannot disagree about which bytes start a line. */
    let mut nl = 0usize;
    let mut at = 0usize;
    while let Some(i) = next_newline(bytes, at) {
        nl += 1;
        at = i + 1;
    }

    let mut starts: Vec<usize> = try_vec_with_capacity(nl + 1)?;

    starts.push(0); /* reserved above; cannot allocate */
    let mut at = 0usize;
    while let Some(i) = next_newline(bytes, at) {
        starts.push(i + 1); /* the next line starts past the newline */
        at = i + 1;
    }

    Some(Lines { starts })
}
