//! The source line table: byte offset -> 1-based line, for `Node#line`.
//!
//! Built once over the parsed input; the parse stamps each element with its
//! start tag's byte offset, and a lookup is a binary search. Shared by every
//! HTML DOM, so it depends on nothing but the bytes.

#![forbid(unsafe_code)]

use crate::falloc::VecPush;

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

/// The high bit of each byte of `w` that is a newline, and no other bit.
///
/// Exact per byte, unlike the shorter `(x - 0x01..) & !x & 0x80..` test, whose
/// borrow can flag the byte above a match: the low seven bits of each byte are
/// added to `0x7f` (a carry into the high bit unless they are all zero), and a
/// byte with its own high bit set is excluded, so a high bit survives only
/// where the byte XORed with `\n` was zero.
#[inline(always)]
fn newline_mask(w: u64) -> u64 {
    const LOW7: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    const NEWLINES: u64 = 0x0a0a_0a0a_0a0a_0a0a;
    let x = w ^ NEWLINES;
    !((x & LOW7).wrapping_add(LOW7) | x | LOW7)
}

/// The eight bytes of `c` (eight long) as one word, first byte lowest.
#[inline(always)]
fn word(c: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(c);
    u64::from_le_bytes(b)
}

/// Build the line table over the input. None on allocation failure, which the
/// caller treats as "no line information" rather than as a parse failure.
///
/// One pass, sixteen bytes at a time: two words tested for a newline at once
/// ([`newline_mask`]), and only a word holding one walked bit by bit. Safe
/// Rust, and faster than the libc `memchr` loop it replaced on any input with
/// newlines in it (that loop paid a call per line, and walked the input twice
/// to size the table first): measured on ~220 KB, 2.3x faster at 2,000 lines
/// and 4x at 6,000, and 9 -> 12 us on a document of one line. (A byte loop,
/// `iter().position()`, measured about 9% off the whole parse: the scan must
/// not be a byte at a time.)
pub fn lines_build(bytes: &[u8]) -> Option<Lines> {
    let mut starts: Vec<usize> = Vec::new();
    starts.falloc_push(0).ok()?;
    let mut chunks = bytes.chunks_exact(16);
    let mut base = 0usize;
    for c in &mut chunks {
        let (lo, hi) = (newline_mask(word(&c[..8])), newline_mask(word(&c[8..])));
        if lo | hi != 0 {
            for (off, mut m) in [(0usize, lo), (8, hi)] {
                while m != 0 {
                    /* The next line starts past the newline. */
                    let at = base + off + (m.trailing_zeros() / 8) as usize;
                    starts.falloc_push(at + 1).ok()?;
                    m &= m - 1;
                }
            }
        }
        base += 16;
    }
    for (i, &b) in chunks.remainder().iter().enumerate() {
        if b == b'\n' {
            starts.falloc_push(base + i + 1).ok()?;
        }
    }
    Some(Lines { starts })
}

#[cfg(test)]
mod tests {
    use super::lines_build;

    /// The table the scan builds is the one a byte loop builds, for every
    /// length around the 16-byte stride, newlines at every offset, and bytes
    /// whose high bit is set (the case a borrowing zero test gets wrong).
    #[test]
    fn the_scan_finds_every_newline_and_nothing_else() {
        for len in 0..80usize {
            for stride in 1..20usize {
                let bytes: Vec<u8> = (0..len)
                    .map(|i| match i % stride {
                        0 => b'\n',
                        1 => 0x8a,
                        2 => 0x0b,
                        _ => b'a' + (i % 7) as u8,
                    })
                    .collect();
                let want: Vec<usize> = core::iter::once(0)
                    .chain(
                        bytes
                            .iter()
                            .enumerate()
                            .filter(|(_, &b)| b == b'\n')
                            .map(|(i, _)| i + 1),
                    )
                    .collect();
                let got = lines_build(&bytes).expect("a table");
                assert_eq!(got.starts, want, "len {len}, stride {stride}");
            }
        }
    }

    #[test]
    fn a_lookup_names_the_line_an_offset_is_on() {
        let lines = lines_build(b"ab\ncd\n\nef").expect("a table");
        assert_eq!(
            [0, 2, 3, 5, 6, 7, 8].map(|o| lines.lookup(o)),
            [1, 1, 2, 2, 3, 4, 4]
        );
    }
}
