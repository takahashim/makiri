//! Browser-compatible UTF-8 input sanitisation.
//!
//! Turns arbitrary bytes into the valid UTF-8 the rest of the engine assumes:
//! every invalid sequence becomes U+FFFD, per WHATWG byte-stream decoding, so
//! parsing never fails on bad bytes and the DOM is always valid UTF-8. Used by
//! the document parse driver and the fragment paths through
//! [`sanitize`].
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
//! # One entry point
//!
//! [`sanitize`] is the whole interface: bytes in, and either those same bytes
//! (valid already - the common case, no copy) or a repaired copy the result
//! owns. The document parse and the fragment parse both go through it, so the
//! "skip when the caller already knows it is valid" rule lives here once.
//!
//! The repaired copy is built in a `cbuf::Buf` and stolen as an `OwnedBuf`,
//! which frees it on drop. That also keeps the growth clamp, the NUL terminator
//! and the `rake oom` injection hook - three properties that a Rust `Vec` would
//! have had to re-earn.

#![forbid(unsafe_code)]

use crate::cbuf::{Buf, BufError, OwnedBuf};
use crate::cutf8::valid;

/// HTML input after browser-compatible decoding: the caller's bytes when they
/// needed no repair, or the repaired copy, which this owns and frees.
pub enum Sanitized<'a> {
    Borrowed(&'a [u8]),
    Owned(OwnedBuf),
}

impl Sanitized<'_> {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Sanitized::Borrowed(b) => b,
            Sanitized::Owned(o) => o.as_slice(),
        }
    }
}

/// UTF-8 -> UTF-8 with every invalid sequence replaced by U+FFFD, into a fresh
/// NUL-terminated buffer.
fn replace_invalid(src: &[u8]) -> Result<OwnedBuf, BufError> {
    /* The output is at most 3x the input: each invalid byte becomes U+FFFD
     * (3 bytes) and valid bytes pass through 1:1. Cap at exactly that bound -
     * tight and tied to the actual input, so a large document still parses but
     * nothing runs away - rather than a blanket ceiling. */
    /* On overflow, `usize::MAX` rather than a restated hard ceiling: every
     * growth path in `cbuf::Buf` already takes min(max, BUF_HARD_MAX), so this
     * is that ceiling, without a second copy of the constant that an
     * `MKR_BUF_HARD_MAX=` build could silently disagree with (see cbuf.rs). */
    let cap = src.len().saturating_mul(3);
    let mut buf = Buf::new(cap);

    /* Pre-size once. A failure here is not fatal: append grows on its own and
     * fails closed if it cannot, so the reserve is a performance hint. */
    let _ = buf.reserve(src.len());

    /* `buf` frees itself on the error returns (`Buf`'s Drop). Each chunk's
     * invalid part is one maximal subpart - or the input's truncated tail -
     * and one U+FFFD stands for all of it, as `String::from_utf8_lossy`. */
    for chunk in src.utf8_chunks() {
        buf.append(chunk.valid().as_bytes())?;
        if !chunk.invalid().is_empty() {
            buf.append("\u{FFFD}".as_bytes())?;
        }
    }

    buf.steal()
}

/// Sanitise `input` for the HTML parser: invalid UTF-8 becomes U+FFFD, valid
/// input is used in place. `known_valid` - the caller already knows the bytes
/// are valid UTF-8, typically from a Ruby String's cached coderange - skips the
/// scan. `None` on OOM, with nothing allocated.
pub fn sanitize(input: &[u8], known_valid: bool) -> Option<Sanitized<'_>> {
    if known_valid || valid(input) {
        return Some(Sanitized::Borrowed(input));
    }
    replace_invalid(input).ok().map(Sanitized::Owned)
}
