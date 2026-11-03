//! Finding the first byte of a set in a slice, sixteen bytes at a time:
//! the one SIMD of the layer, for a reader that scans text for the few bytes
//! it must act on (a JSON generator for the bytes to escape, a JSON parser
//! for the end of a string).
//!
//! Ruby-free, as `utf8_copy` is. The vector paths are the baseline of their
//! target - NEON on aarch64, SSE2 on x86_64, both always present there - so
//! nothing is detected at run time; elsewhere the scan is eight bytes at a
//! time in plain Rust. The one `unsafe` per path is the 16-byte load, whose
//! bounds the loop condition holds; the tests hold every path to the scalar
//! definition, byte for byte, over random input.

#![allow(unsafe_code)]

/// A set of bytes: every byte below `below`, the bytes of `eq`, and - with
/// `high` - every byte from 0x80 up.
#[derive(Clone, Copy, Debug)]
pub struct ByteSet {
    below: u8,
    eq: [u8; 4],
    n_eq: usize,
    high: bool,
    /// Every byte's membership, for the bytes outside a whole block: one
    /// load a byte, as a scanner of short text needs.
    table: [bool; 256],
}

impl ByteSet {
    /// The bytes below `below` (0 for none), the bytes of `eq` (at most
    /// four; more is a mistake caught here), and every byte past ASCII when
    /// `high`.
    pub const fn new(below: u8, eq: &[u8], high: bool) -> ByteSet {
        assert!(eq.len() <= 4, "a ByteSet takes at most four bytes to match");
        let mut e = [0u8; 4];
        let mut i = 0;
        while i < eq.len() {
            e[i] = eq[i];
            i += 1;
        }
        /* The unused slots repeat the first byte (or a byte `below`
         * already holds), so a vector compare against them adds nothing. */
        while i < 4 {
            e[i] = if eq.is_empty() { 0 } else { eq[0] };
            i += 1;
        }
        let mut table = [false; 256];
        let mut b = 0;
        while b < 256 {
            /* `b` < 256, so the byte is exact; a `const fn` has no `TryFrom`. */
            #[allow(clippy::cast_possible_truncation, reason = "b < 256, the loop's bound")]
            #[cfg_attr(
                dylint_lib = "makiri_lints",
                allow(
                    c_integer_outside_int,
                    reason = "a byte index below 256, in a const fn, which has no TryFrom"
                )
            )]
            let c = b as u8;
            let mut hit = c < below || (high && c >= 0x80);
            let mut k = 0;
            while k < eq.len() {
                hit = hit || eq[k] == c;
                k += 1;
            }
            table[b] = hit;
            b += 1;
        }
        ByteSet {
            below,
            eq: e,
            n_eq: eq.len(),
            high,
            table,
        }
    }

    /// Whether `b` is in the set.
    #[inline(always)]
    pub fn contains(&self, b: u8) -> bool {
        self.table[usize::from(b)]
    }
}

/// The first index from `from` whose byte is in `set`, or `s.len()` when
/// none is (and `from` past the end answers `s.len()`).
#[inline(always)]
pub fn find(s: &[u8], from: usize, set: &ByteSet) -> usize {
    let mut i = from.min(s.len());
    i = vector_skip(s, i, set);
    while let Some(&b) = s.get(i) {
        if set.contains(b) {
            return i;
        }
        i += 1;
    }
    i
}

/// Past every whole 16-byte block from `i` that holds no byte of `set`; the
/// index of the first block that does (or of the tail), to finish byte by
/// byte.
#[cfg(target_arch = "aarch64")]
#[inline]
fn vector_skip(s: &[u8], mut i: usize, set: &ByteSet) -> usize {
    use core::arch::aarch64::*;
    // SAFETY: NEON is part of every aarch64 target Rust supports; these
    // broadcast a byte into a register.
    let (below, e0, e1, e2, e3, high) = unsafe {
        (
            vdupq_n_u8(set.below),
            vdupq_n_u8(set.eq[0]),
            vdupq_n_u8(set.eq[1]),
            vdupq_n_u8(set.eq[2]),
            vdupq_n_u8(set.eq[3]),
            vdupq_n_u8(if set.high { 0x80 } else { 0xff }),
        )
    };
    let any_eq = set.n_eq > 0;
    let high_on = set.high;
    while i + 16 <= s.len() {
        // SAFETY: `i + 16 <= s.len()`, so the 16 bytes at `i` are in `s`;
        // `vld1q_u8` takes any alignment. The rest is register arithmetic.
        let hit = unsafe {
            let v = vld1q_u8(s.as_ptr().add(i));
            let mut m = vcltq_u8(v, below);
            if any_eq {
                m = vorrq_u8(m, vceqq_u8(v, e0));
                m = vorrq_u8(m, vceqq_u8(v, e1));
                m = vorrq_u8(m, vceqq_u8(v, e2));
                m = vorrq_u8(m, vceqq_u8(v, e3));
            }
            if high_on {
                m = vorrq_u8(m, vcgeq_u8(v, high));
            }
            vmaxvq_u8(m) != 0
        };
        if hit {
            return i;
        }
        i += 16;
    }
    i
}

#[cfg(target_arch = "x86_64")]
#[inline]
#[allow(
    clippy::cast_possible_wrap,
    reason = "SSE2 takes bytes as i8: the bits are the byte's, reinterpreted"
)]
#[cfg_attr(
    dylint_lib = "makiri_lints",
    allow(
        c_integer_outside_int,
        reason = "a byte reinterpreted as SSE2's i8 lane, bit for bit"
    )
)]
fn vector_skip(s: &[u8], mut i: usize, set: &ByteSet) -> usize {
    use core::arch::x86_64::*;
    /* SSE2 compares are signed, so "below" and "high" are asked of the
     * bytes shifted by 0x80 (an unsigned compare made signed). */
    // SAFETY: SSE2 is part of every x86_64 target; these broadcast a byte.
    let (bias, below, e0, e1, e2, e3) = unsafe {
        (
            _mm_set1_epi8(i8::MIN),
            _mm_set1_epi8((set.below ^ 0x80) as i8),
            _mm_set1_epi8(set.eq[0] as i8),
            _mm_set1_epi8(set.eq[1] as i8),
            _mm_set1_epi8(set.eq[2] as i8),
            _mm_set1_epi8(set.eq[3] as i8),
        )
    };
    let any_eq = set.n_eq > 0;
    let high_on = set.high;
    while i + 16 <= s.len() {
        // SAFETY: `i + 16 <= s.len()`, so the 16 bytes at `i` are in `s`;
        // `_mm_loadu_si128` takes any alignment.
        let hit = unsafe {
            let v = _mm_loadu_si128(s.as_ptr().add(i).cast::<__m128i>());
            let biased = _mm_xor_si128(v, bias);
            let mut m = _mm_cmplt_epi8(biased, below);
            if any_eq {
                m = _mm_or_si128(m, _mm_cmpeq_epi8(v, e0));
                m = _mm_or_si128(m, _mm_cmpeq_epi8(v, e1));
                m = _mm_or_si128(m, _mm_cmpeq_epi8(v, e2));
                m = _mm_or_si128(m, _mm_cmpeq_epi8(v, e3));
            }
            if high_on {
                /* A byte from 0x80 up has its top bit set. */
                m = _mm_or_si128(m, v);
            }
            _mm_movemask_epi8(m) != 0
        };
        if hit {
            return i;
        }
        i += 16;
    }
    i
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
#[inline]
fn vector_skip(s: &[u8], i: usize, set: &ByteSet) -> usize {
    swar_skip(s, i, set)
}

/// [`vector_skip`] eight bytes at a time in plain Rust: the path of every
/// other target, compiled everywhere so the tests hold it too.
#[cfg_attr(
    any(target_arch = "aarch64", target_arch = "x86_64"),
    allow(
        dead_code,
        reason = "the vector paths stand in for it here; the tests still run it"
    )
)]
#[inline]
fn swar_skip(s: &[u8], mut i: usize, set: &ByteSet) -> usize {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGHS: u64 = 0x8080_8080_8080_8080;
    let has_less =
        |x: u64, n: u8| n != 0 && x.wrapping_sub(ONES.wrapping_mul(u64::from(n))) & !x & HIGHS != 0;
    let has_byte = |x: u64, b: u8| has_less(x ^ ONES.wrapping_mul(u64::from(b)), 1);
    while let Some(chunk) = s.get(i..i + 8) {
        let mut word = [0u8; 8];
        word.copy_from_slice(chunk);
        let x = u64::from_le_bytes(word);
        let hit = (set.below > 0x80 || has_less(x, set.below))
            || (set.high && x & HIGHS != 0)
            || set.eq.iter().take(set.n_eq).any(|&e| has_byte(x, e));
        if hit {
            return i;
        }
        i += 8;
    }
    i
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::cast_possible_truncation,
    reason = "test input, built freely"
)]
mod tests {
    use super::{find, swar_skip, ByteSet};

    /// [`find`] over the plain-Rust path instead of the vector one.
    fn find_swar(s: &[u8], from: usize, set: &ByteSet) -> usize {
        let mut i = swar_skip(s, from.min(s.len()), set);
        while let Some(&b) = s.get(i) {
            if set.contains(b) {
                return i;
            }
            i += 1;
        }
        i
    }

    /// The definition, from the set's own parts rather than its table.
    fn scalar(s: &[u8], from: usize, set: &ByteSet) -> usize {
        let member = |b: u8| {
            b < set.below
                || (set.high && b >= 0x80)
                || set.eq.iter().take(set.n_eq).any(|&e| e == b)
        };
        (from.min(s.len())..s.len())
            .find(|&i| member(s[i]))
            .unwrap_or(s.len())
    }

    #[test]
    fn every_path_agrees_with_the_definition() {
        let sets = [
            ByteSet::new(0x20, b"\"\\", false),
            ByteSet::new(0x20, b"\"\\/\xe2", false),
            ByteSet::new(0x20, b"\"\\", true),
            ByteSet::new(0x20, b"\"\\/\xe2", true),
            ByteSet::new(0, b"\"\\", false),
            ByteSet::new(0, b"", true),
            ByteSet::new(0, b"", false),
            ByteSet::new(0x80, b"", false),
            ByteSet::new(0xff, b"\xff", false),
        ];
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..40_000 {
            let len = (next() % 80) as usize;
            /* Mostly plain bytes, with a few of every kind: a match lands
             * anywhere in a block, at a block's edge, or nowhere. */
            let s: Vec<u8> = (0..len)
                .map(|_| {
                    let r = next();
                    match r % 23 {
                        0 => (r >> 8) as u8,
                        1 => b'"',
                        2 => b'\\',
                        3 => b'/',
                        4 => 0xe2,
                        5 => (r >> 8) as u8 & 0x1f,
                        _ => b'a' + (r >> 8) as u8 % 26,
                    }
                })
                .collect();
            for set in &sets {
                for from in [0, 1, 7, 15, 16, 17, len.saturating_sub(1), len, len + 3] {
                    let want = scalar(&s, from, set);
                    assert_eq!(find(&s, from, set), want, "{s:?} from {from} {set:?}");
                    assert_eq!(
                        find_swar(&s, from, set),
                        want,
                        "swar {s:?} from {from} {set:?}"
                    );
                }
            }
        }
    }
}
