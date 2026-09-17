//! Browser-compatible UTF-8 input sanitisation (dom_adapter/utf8_input.c).
//!
//! Turns arbitrary bytes into the valid UTF-8 the rest of the engine assumes:
//! every invalid sequence becomes U+FFFD, per WHATWG byte-stream decoding, so
//! parsing never fails on bad bytes and the DOM is always valid UTF-8. Used by
//! the document parse driver and the fragment paths through
//! [`utf8_sanitize`].
//!
//! # Lexbor's encoder is gone from this path
//!
//! The C ran Lexbor's decode/encode pipeline (`lxb_encoding_data`, a codepoint
//! buffer, an output buffer, the SMALL_BUFFER loop) to do the replacement.
//! `String::from_utf8_lossy` is the same algorithm, so this file depends on no
//! Lexbor symbol at all.
//!
//! "The same algorithm" is a claim, and lossy UTF-8 decoders genuinely differ on
//! where one replacement character ends and the next begins - `E1 80 41` is one
//! U+FFFD plus `A` under the maximal-subpart rule and two U+FFFD under a
//! byte-at-a-time one. So it was measured, not assumed: ~20,000 random byte
//! strings and 2,200 enumerated edge cases (every 1- and 2-byte input, every
//! 3- and 4-byte lead against the continuation-range boundaries, the overlongs,
//! the surrogate range, the beyond-U+10FFFF leads, the truncated sequences, and
//! each landmark followed by an ASCII byte and by a continuation byte) produced
//! byte-identical output from both. `spec/utf8_sanitize_spec.rb` keeps the
//! enumerated half as a standing check.
//!
//! # One spelling of the signature
//!
//! The sanitised result is an owned buffer the caller frees with libc `free`.
//! It used to be returned through two out-parameters (`*mut *mut u8`, `*mut
//! usize`) to match the C's `lxb_char_t **`; the two callers wrapped it in a
//! `Drop` guard each, so the ownership was already Rust's. [`Sanitized`] now
//! carries it in the type and neither caller has to remember.
//!
//! # The allocation stays C's
//!
//! The result is a `malloc`'d buffer the caller frees with libc `free`, so it is
//! built in an `mkr_buf_t` and stolen, exactly as before. That also keeps the
//! growth clamp, the NUL terminator and the `rake oom` injection hook - three
//! properties that a Rust `Vec` and a hand-written `malloc` would each have had
//! to re-earn.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)]

use crate::cbuf::{Buf, BufError, OwnedBuf};
use crate::cutf8::valid;

/// The sanitiser's replacement buffer: `malloc`'d, NUL-terminated, and owned by
/// the caller, who frees it with libc `free`.
/// What [`utf8_sanitize`] decided about the input.
pub enum Sanitized {
    /// Already valid UTF-8: the caller parses the input in place, no copy.
    Unchanged,
    /// Invalid bytes were replaced with U+FFFD; a fresh buffer the caller owns.
    Replaced(OwnedBuf),
}

/// UTF-8 -> UTF-8 with every invalid sequence replaced by U+FFFD, into a freshly
/// `malloc`'d, NUL-terminated buffer. NULL on OOM.
fn replace_invalid(src: &[u8]) -> Result<OwnedBuf, BufError> {
    /* The output is at most 3x the input: each invalid byte becomes U+FFFD
     * (3 bytes) and valid bytes pass through 1:1. Cap at exactly that bound -
     * tight and tied to the actual input, so a large document still parses but
     * nothing runs away - rather than a blanket ceiling. */
    /* On overflow, `usize::MAX` rather than a restated MKR_BUF_HARD_MAX: every
     * growth path in mkr_buf.c already takes min(max, HARD_MAX), so this is the
     * same ceiling the C reached for, without a Rust copy of the constant that a
     * `-DMKR_BUF_HARD_MAX=` build could silently disagree with (see cbuf.rs). */
    let cap = src.len().saturating_mul(3);
    let mut buf = Buf::new(cap);

    /* Pre-size once. A failure here is not fatal: append grows on its own and
     * fails closed if it cannot, so the reserve is a performance hint. */
    let _ = buf.reserve(src.len());

    let mut rest = src;
    loop {
        match core::str::from_utf8(rest) {
            Ok(s) => {
                if append(&mut buf, s.as_bytes()).is_err() {
                    return Err(BufError::Oom);
                }
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                if append(&mut buf, &rest[..good]).is_err() {
                    return Err(BufError::Oom);
                }
                if append(&mut buf, "\u{FFFD}".as_bytes()).is_err() {
                    return Err(BufError::Oom);
                }
                match e.error_len() {
                    /* A maximal subpart of `n` bytes was invalid; one U+FFFD
                     * stands for all of it and decoding resumes after it. */
                    Some(n) => rest = &rest[good + n..],
                    /* The input ended mid-sequence: one U+FFFD for the tail,
                     * and there is nothing left. */
                    None => break,
                }
            }
        }
    }

    buf.steal()
}

/// Append, freeing the buffer on failure so the error path leaks nothing.
#[inline]
fn append(buf: &mut Buf, bytes: &[u8]) -> Result<(), ()> {
    if bytes.is_empty() {
        return Ok(());
    }
    if buf.append(bytes).is_err() {
        buf.free();
        return Err(());
    }
    Ok(())
}

/// Sanitise `src` for the HTML parser.
///
/// `Unchanged` when the input is already valid UTF-8 (the common case), so the
/// caller parses `src` as-is with no copy. `Replaced` carries a freshly
/// `malloc`'d, NUL-terminated replacement the caller owns. `None` on OOM, with
/// nothing allocated.
pub unsafe fn utf8_sanitize(src: *const u8, len: usize) -> Option<Sanitized> {
    if src.is_null() || len == 0 || valid(core::slice::from_raw_parts(src, len)) {
        return Some(Sanitized::Unchanged);
    }
    replace_invalid(core::slice::from_raw_parts(src, len))
        .ok()
        .map(Sanitized::Replaced)
}
