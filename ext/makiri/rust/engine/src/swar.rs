//! Word-at-a-time byte tests (SWAR): the HTML tokenizer's `seek4` and the
//! line table's newline scan read eight bytes per step through these.
//!
//! The bit tricks are proved in `verus/swar.rs` and `verus/source_lines.rs`
//! (`rake verus`), which copy these functions; `rake verus:sync` checks the
//! copies against this file.

#![forbid(unsafe_code)]

/// `0x01` in every byte.
pub const ONES: u64 = u64::MAX / 0xFF;
/// `0x80` in every byte.
pub const HIGH: u64 = ONES * 0x80;

/// The first eight bytes of `b` as a little-endian word. Written as shifts
/// rather than `u64::from_le_bytes`, which Verus has no contract for; LLVM
/// folds it into one load. Panics when `b` is shorter. A caller slices `b`
/// first (`&s[at..at + 8]`, or a rest it advances): an index computed here
/// keeps its bounds check in the loop, 20% on the line scan.
#[inline(always)]
pub fn word(b: &[u8]) -> u64 {
    u64::from(b[0])
        | u64::from(b[1]) << 8
        | u64::from(b[2]) << 16
        | u64::from(b[3]) << 24
        | u64::from(b[4]) << 32
        | u64::from(b[5]) << 40
        | u64::from(b[6]) << 48
        | u64::from(b[7]) << 56
}

/// The high bit of the LOWEST zero byte of `v` is set, and no bit below it.
/// Above it the answer is noise: the borrow out of a zero byte can mark the
/// byte after it (`0x0100` has one). So a caller may trust only the lowest
/// set bit, and a zero answer, which means no byte of `v` is zero.
#[inline(always)]
pub fn has_zero(v: u64) -> u64 {
    v.wrapping_sub(ONES) & !v & HIGH
}
