//! Browser-compatible UTF-8 input sanitisation (dom_adapter/utf8_input.c).
//!
//! Turns arbitrary bytes into the valid UTF-8 the rest of the engine assumes:
//! every invalid sequence becomes U+FFFD, per WHATWG byte-stream decoding, so
//! parsing never fails on bad bytes and the DOM is always valid UTF-8. Used by
//! the document parse driver and the fragment paths through
//! [`mkr_utf8_sanitize`].
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
//! # The allocation stays C's
//!
//! The result is handed to a C caller that `free()`s it, so it is built in an
//! `mkr_buf_t` and stolen, exactly as before. That also keeps the growth clamp,
//! the NUL terminator and the `rake oom` injection hook - three properties that
//! a Rust `Vec` and a hand-written `malloc` would each have had to re-earn.

#![allow(clippy::missing_safety_doc)]

use core::ffi::c_int;

use crate::cbuf::{mkr_buf_append, mkr_buf_reserve, mkr_buf_steal, Buf, MKR_OK};

extern "C" {
    /// The one UTF-8 validator (core/mkr_utf8.c), shared with the Ruby bridge's
    /// strict input gate so both answer the same question. Its contract is that
    /// `true` means the replacement below would be a no-op, which is what makes
    /// the short-circuit sound.
    fn mkr_utf8_valid(src: *const u8, len: usize) -> bool;
}

/// UTF-8 -> UTF-8 with every invalid sequence replaced by U+FFFD, into a freshly
/// `malloc`'d, NUL-terminated buffer. NULL on OOM.
unsafe fn replace_invalid(src: &[u8], out_len: *mut usize) -> *mut core::ffi::c_char {
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
    let _ = mkr_buf_reserve(&mut buf, src.len());

    let mut rest = src;
    loop {
        match core::str::from_utf8(rest) {
            Ok(s) => {
                if append(&mut buf, s.as_bytes()).is_err() {
                    return core::ptr::null_mut();
                }
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                if append(&mut buf, &rest[..good]).is_err() {
                    return core::ptr::null_mut();
                }
                if append(&mut buf, "\u{FFFD}".as_bytes()).is_err() {
                    return core::ptr::null_mut();
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

    mkr_buf_steal(&mut buf, out_len)
}

/// Append, freeing the buffer on failure so the error path leaks nothing.
#[inline]
unsafe fn append(buf: &mut Buf, bytes: &[u8]) -> Result<(), ()> {
    if bytes.is_empty() {
        return Ok(());
    }
    if mkr_buf_append(buf, bytes.as_ptr() as *const core::ffi::c_void, bytes.len()) != MKR_OK {
        buf.free();
        return Err(());
    }
    Ok(())
}

/// Sanitise `src` for the HTML parser.
///
/// Sets `*out` to NULL and returns 0 when the input is already valid UTF-8 (the
/// common case), which tells the caller to use `src` as-is with no copy.
/// Otherwise `*out` receives a freshly `malloc`'d, NUL-terminated replacement
/// the caller owns and `free()`s, with `*out_len` its length. Returns -1 on OOM.
#[no_mangle]
pub unsafe extern "C" fn mkr_utf8_sanitize(
    src: *const u8,
    len: usize,
    out: *mut *mut core::ffi::c_char,
    out_len: *mut usize,
) -> c_int {
    *out = core::ptr::null_mut();
    *out_len = 0;
    if src.is_null() || len == 0 || mkr_utf8_valid(src, len) {
        return 0;
    }
    *out = replace_invalid(core::slice::from_raw_parts(src, len), out_len);
    if (*out).is_null() {
        -1
    } else {
        0
    }
}
